use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use anyhow::{Context, bail, ensure};
use libloading::Library;
use spacetimedb_lib::ConnectionId;
use spacetimedb_primitives::{ColId, IndexId, TableId, errno};

use crate::host::instance_env::{ChunkPool, InstanceEnv};
use crate::host::wasm_common::{
    FuncNames, RowIterIdx, RowIters, TimingSpan, TimingSpanIdx, TimingSpanSet, err_to_errno,
};

type Dispatch = unsafe extern "C" fn(*mut c_void, u32, *const usize, usize) -> i64;
type Entry = unsafe extern "C" fn(Dispatch, *mut c_void, *const u64, bool) -> i16;

#[derive(Clone, serde::Deserialize)]
pub(super) struct NativeConfig {
    library: PathBuf,
    native_blake3: String,
    wasm_blake3: String,
    #[serde(skip)]
    claimed: Arc<AtomicBool>,
}

impl NativeConfig {
    pub(super) fn for_program(program: &[u8]) -> anyhow::Result<Option<Self>> {
        let Some(path) = std::env::var_os("LYRACORE_NATIVE_DATABASE_MANIFEST") else {
            return Ok(None);
        };
        let config: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        ensure!(
            blake3::hash(program).to_hex().as_str() == config.wasm_blake3,
            "native manifest does not match the published Wasm"
        );
        Ok(Some(config))
    }
}

pub(super) struct NativeInstance {
    entry: Entry,
    description: Vec<u8>,
    pool: ChunkPool,
    reported: bool,
    _library: Library,
    _file: tempfile::NamedTempFile,
}

impl NativeInstance {
    pub(super) fn load(
        config: &NativeConfig,
        env: &InstanceEnv,
        names: &FuncNames,
    ) -> anyhow::Result<Option<Self>> {
        if config.claimed.swap(true, Ordering::AcqRel) {
            env.console_log_simple_message(
                1_u8.into(),
                None,
                "LYRACORE_NATIVE_DATABASE_FALLBACK: replacement uses Wasm",
            );
            return Ok(None);
        }
        let bytes = std::fs::read(&config.library)?;
        ensure!(
            blake3::hash(&bytes).to_hex().as_str() == config.native_blake3,
            "native artifact hash mismatch"
        );
        // A distinct inode gives every instance its own SDK caches and mutable library globals.
        let mut file = tempfile::Builder::new()
            .prefix("lyracore-native-")
            .suffix(".so")
            .tempfile()?;
        std::io::Write::write_all(&mut file, &bytes)?;
        let library = unsafe { Library::new(file.path()) }?;
        for name in &names.preinits {
            let preinit = unsafe { library.get::<unsafe extern "C" fn()>(name.as_bytes()) }?;
            unsafe { preinit() };
        }
        let entry = *unsafe { library.get::<Entry>(b"__lyracore_native_v1") }?;
        let mut scope = CallScope::new(env, &[], ChunkPool::default());
        let code = unsafe {
            entry(
                dispatch,
                (&mut scope as *mut CallScope<'_>).cast(),
                std::ptr::null(),
                true,
            )
        };
        scope.check(code)?;
        ensure!(code == 0, "native description failed");
        Ok(Some(Self {
            entry,
            description: scope.sink,
            pool: scope.pool,
            reported: false,
            _library: library,
            _file: file,
        }))
    }

    pub(super) fn validate(&self, reference: &[u8]) -> anyhow::Result<()> {
        ensure!(
            self.description == reference,
            "native and Wasm Module descriptions differ"
        );
        Ok(())
    }

    pub(super) fn call(
        &mut self,
        env: &InstanceEnv,
        args: &[u8],
        mut context: [u64; 10],
    ) -> anyhow::Result<(i32, Vec<u8>)> {
        if !self.reported {
            env.console_log_simple_message(
                2.into(),
                env.log_record_function(),
                "LYRACORE_NATIVE_DATABASE_V1",
            );
            self.reported = true;
        }
        let mut scope = CallScope::new(env, args, std::mem::take(&mut self.pool));
        context[8] = if args.is_empty() { 0 } else { 1 };
        context[9] = 1;
        let code = unsafe {
            (self.entry)(
                dispatch,
                (&mut scope as *mut CallScope<'_>).cast(),
                context.as_ptr(),
                false,
            )
        };
        self.pool = std::mem::take(&mut scope.pool);
        scope.check(code)?;
        Ok((code.into(), scope.sink))
    }
}

struct CallScope<'a> {
    env: &'a InstanceEnv,
    sources: std::collections::BTreeMap<u32, bytes::Bytes>,
    next_source: u32,
    sink: Vec<u8>,
    iters: RowIters,
    timers: TimingSpanSet,
    pool: ChunkPool,
    fault: Option<String>,
    started: Instant,
    calls: u32,
}

impl<'a> CallScope<'a> {
    fn new(env: &'a InstanceEnv, args: &[u8], pool: ChunkPool) -> Self {
        Self {
            env,
            sources: [(1, bytes::Bytes::copy_from_slice(args))].into(),
            next_source: 2,
            sink: Vec::new(),
            iters: RowIters::default(),
            timers: TimingSpanSet::default(),
            pool,
            fault: None,
            started: Instant::now(),
            calls: 0,
        }
    }

    fn check(&self, code: i16) -> anyhow::Result<()> {
        if let Some(fault) = &self.fault {
            bail!("native host trap: {fault}");
        }
        ensure!(code != -2, "native reducer panicked");
        Ok(())
    }

    unsafe fn dispatch(&mut self, operation: u32, a: &[usize]) -> anyhow::Result<i64> {
        const LENGTHS: [usize; 28] = [
            3, 3, 2, 2, 9, 9, 4, 4, 3, 1, 3, 4, 4, 2, 8, 1, 3, 3, 2, 2, 2, 1, 4, 1, 1, 0, 0, 5,
        ];
        ensure!(
            LENGTHS.get(operation as usize) == Some(&a.len()),
            "native ABI argument mismatch"
        );
        self.calls += 1;
        // This bounds host work only; native code without host calls has no Wasmtime fuel limit.
        ensure!(
            self.calls <= 2_000_000
                && (self.calls % 1024 != 0 || self.started.elapsed() < Duration::from_secs(10)),
            "native host call limit exceeded"
        );
        let env = self.env;
        match operation {
            0 | 1 => {
                let name = std::str::from_utf8(unsafe { input(a[0], a[1]) })?;
                let id = if operation == 0 {
                    env.table_id_from_name(name)?.0
                } else {
                    env.index_id_from_name(name)?.0
                };
                unsafe { write(a[2], id) };
            }
            2 => unsafe { write(a[1], env.datastore_table_row_count(TableId(a[0] as u32))?) },
            3 => {
                let chunks =
                    env.datastore_table_scan_bsatn_chunks(&mut self.pool, TableId(a[0] as u32))?;
                unsafe { write(a[1], self.iters.insert(chunks.into_iter()).0) };
            }
            4 | 5 => {
                let (prefix, start, end) =
                    unsafe { (input(a[1], a[2]), input(a[4], a[5]), input(a[6], a[7])) };
                let index = IndexId(a[0] as u32);
                let count = ColId(a[3] as u16);
                let result = if operation == 4 {
                    let chunks = env.datastore_index_scan_range_bsatn_chunks(
                        &mut self.pool,
                        index,
                        prefix,
                        count,
                        start,
                        end,
                    )?;
                    self.iters.insert(chunks.into_iter()).0
                } else {
                    env.datastore_delete_by_index_scan_range_bsatn(
                        index, prefix, count, start, end,
                    )?
                };
                unsafe { write(a[8], result) };
            }
            6 | 7 => {
                let point = unsafe { input(a[1], a[2]) };
                let index = IndexId(a[0] as u32);
                let result = if operation == 6 {
                    let chunks =
                        env.datastore_index_scan_point_bsatn_chunks(&mut self.pool, index, point)?;
                    self.iters.insert(chunks.into_iter()).0
                } else {
                    env.datastore_delete_by_index_scan_point_bsatn(index, point)?
                };
                unsafe { write(a[3], result) };
            }
            8 => {
                let id = RowIterIdx(a[0] as u32);
                let Some(iter) = self.iters.get_mut(id) else {
                    return Ok(errno::NO_SUCH_ITER.get().into());
                };
                let len = unsafe { (a[2] as *mut usize).as_mut().context("null length")? };
                let buffer = unsafe { output(a[1], *len) };
                let written = InstanceEnv::fill_buffer_from_iter(iter, buffer, &mut self.pool);
                match (written, iter.as_slice().first()) {
                    (0, Some(chunk)) => {
                        *len = chunk.len();
                        return Ok(errno::BUFFER_TOO_SMALL.get().into());
                    }
                    (_, None) => {
                        self.iters.take(id);
                        *len = written;
                        return Ok(-1);
                    }
                    _ => {
                        *len = written;
                    }
                }
            }
            9 => {
                if self.iters.take(RowIterIdx(a[0] as u32)).is_none() {
                    return Ok(errno::NO_SUCH_ITER.get().into());
                }
            }
            10 | 11 => {
                let offset = usize::from(operation == 11);
                let len = unsafe {
                    (a[2 + offset] as *mut usize)
                        .as_mut()
                        .context("null row length")?
                };
                let row = unsafe { output(a[1 + offset], *len) };
                *len = if operation == 10 {
                    env.insert(TableId(a[0] as u32), row)?
                } else {
                    env.update(TableId(a[0] as u32), IndexId(a[1] as u32), row)?
                };
            }
            12 => unsafe {
                write(
                    a[3],
                    env.datastore_delete_all_by_eq_bsatn(TableId(a[0] as u32), input(a[1], a[2]))?,
                )
            },
            13 => unsafe { write(a[1], env.clear(TableId(a[0] as u32))?) },
            14 => {
                let target =
                    (a[1] != 0).then(|| String::from_utf8_lossy(unsafe { input(a[1], a[2]) }));
                let filename =
                    (a[3] != 0).then(|| String::from_utf8_lossy(unsafe { input(a[3], a[4]) }));
                let message = String::from_utf8_lossy(unsafe { input(a[6], a[7]) });
                let record = crate::database_logger::Record {
                    ts: InstanceEnv::now_for_logging(),
                    target: target.as_deref(),
                    filename: filename.as_deref(),
                    line_number: (a[5] as u32 != u32::MAX).then_some(a[5] as u32),
                    function: env.log_record_function(),
                    message: &message,
                };
                env.console_log((a[0] as u8).into(), &record, &());
            }
            15 => unsafe {
                output(a[0], 32).copy_from_slice(&env.database_identity().to_byte_array())
            },
            16 => {
                if a[0] != 1 {
                    return Ok(errno::NO_SUCH_BYTES.get().into());
                }
                let len = unsafe { *(a[2] as *const usize) };
                self.sink.extend_from_slice(unsafe { input(a[1], len) });
            }
            17 => {
                let id = a[0] as u32;
                let Some(source) = self.sources.get_mut(&id) else {
                    return Ok(errno::NO_SUCH_BYTES.get().into());
                };
                let len = unsafe {
                    (a[2] as *mut usize)
                        .as_mut()
                        .context("null source length")?
                };
                *len = (*len).min(source.len());
                unsafe { output(a[1], *len).copy_from_slice(&source.split_to(*len)) };
                if source.is_empty() {
                    self.sources.remove(&id);
                    return Ok(-1);
                }
            }
            18 => {
                let Some(source) = self.sources.get(&(a[0] as u32)) else {
                    return Ok(errno::NO_SUCH_BYTES.get().into());
                };
                unsafe { write(a[1], u32::try_from(source.len())?) };
            }
            19 => {
                let connection =
                    ConnectionId::from_le_byte_array(unsafe { input(a[0], 16) }.try_into()?);
                let id = match env.get_jwt_payload(connection)? {
                    None => 0,
                    Some(jwt) => {
                        let id = self.next_source;
                        self.next_source += 1;
                        self.sources.insert(id, jwt.into());
                        id
                    }
                };
                unsafe { write(a[1], id) };
            }
            20 => {
                let name = String::from_utf8_lossy(unsafe { input(a[0], a[1]) }).into_owned();
                return Ok(self.timers.insert(TimingSpan::new(name)).0.into());
            }
            21 => {
                let Some(span) = self.timers.take(TimingSpanIdx(a[0] as u32)) else {
                    bail!("unknown native timing span");
                };
                env.console_timer_end(&span, env.log_record_function());
            }
            _ => bail!("native host operation {operation} is unsupported"),
        }
        Ok(0)
    }
}

// All pointers belong to the matching library and remain valid for this synchronous callback.
unsafe extern "C" fn dispatch(
    context: *mut c_void,
    operation: u32,
    args: *const usize,
    count: usize,
) -> i64 {
    let scope = unsafe { &mut *context.cast::<CallScope<'_>>() };
    if scope.fault.is_some() {
        return errno::HOST_CALL_FAILURE.get().into();
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        scope.dispatch(operation, std::slice::from_raw_parts(args, count))
    }));
    match result {
        Ok(Ok(code)) => code,
        Ok(Err(error)) => {
            match error.downcast::<crate::error::NodesError>() {
                Ok(error) => match err_to_errno(error) {
                    Ok((code, None)) => return code.get().into(),
                    Ok((_, Some(message))) => scope.fault = Some(message),
                    Err(error) => scope.fault = Some(error.to_string()),
                },
                Err(error) => scope.fault = Some(error.to_string()),
            }
            errno::HOST_CALL_FAILURE.get().into()
        }
        Err(_) => {
            scope.fault = Some("host callback panicked".into());
            errno::HOST_CALL_FAILURE.get().into()
        }
    }
}

unsafe fn input<'a>(pointer: usize, len: usize) -> &'a [u8] {
    if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(pointer as *const u8, len) }
    }
}

unsafe fn output<'a>(pointer: usize, len: usize) -> &'a mut [u8] {
    if len == 0 {
        &mut []
    } else {
        unsafe { std::slice::from_raw_parts_mut(pointer as *mut u8, len) }
    }
}

unsafe fn write<T>(pointer: usize, value: T) {
    unsafe { (pointer as *mut T).write(value) };
}
