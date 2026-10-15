use std::ptr;

use spacetimedb_primitives::errno;

use crate::with_storage;

// Pointer and buffer contracts follow spacetimedb-bindings-sys 2.7.1, spacetime_10.x.
unsafe fn bytes<'a>(data: *const u8, len: usize) -> &'a [u8] {
    if len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(data, len) }
    }
}

unsafe fn output<T>(out: *mut T, result: Result<T, u16>) -> u16 {
    match result {
        Ok(value) => {
            unsafe {
                out.write(value);
            }
            0
        }
        Err(code) => code,
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn table_id_from_name(name: *const u8, len: usize, out: *mut u32) -> u16 {
    let result = std::str::from_utf8(unsafe { bytes(name, len) })
        .map_err(|_| 1u16)
        .and_then(|name| with_storage(|storage| storage.table_id(name)));
    unsafe { output(out, result) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn index_id_from_name(name: *const u8, len: usize, out: *mut u32) -> u16 {
    let result = std::str::from_utf8(unsafe { bytes(name, len) })
        .map_err(|_| 1u16)
        .and_then(|name| with_storage(|storage| storage.index_id(name)));
    unsafe { output(out, result) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn datastore_table_row_count(table: u32, out: *mut u64) -> u16 {
    unsafe { output(out, with_storage(|storage| storage.count(table))) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn datastore_table_scan_bsatn(table: u32, out: *mut u32) -> u16 {
    unsafe { output(out, with_storage(|storage| storage.scan(table))) }
}

macro_rules! range_operation {
    ($name:ident, $method:ident) => {
        #[unsafe(no_mangle)]
        unsafe extern "C" fn $name(
            index: u32,
            prefix: *const u8,
            prefix_len: usize,
            count: u16,
            start: *const u8,
            start_len: usize,
            end: *const u8,
            end_len: usize,
            out: *mut u32,
        ) -> u16 {
            let result = with_storage(|storage| {
                let rows = storage.range(
                    index,
                    unsafe { bytes(prefix, prefix_len) },
                    count as usize,
                    unsafe { bytes(start, start_len) },
                    unsafe { bytes(end, end_len) },
                )?;
                storage.$method(index, rows)
            });
            unsafe { output(out, result) }
        }
    };
}

range_operation!(datastore_index_scan_range_bsatn, scan_index);
range_operation!(datastore_btree_scan_bsatn, scan_index);
range_operation!(datastore_delete_by_index_scan_range_bsatn, delete_index);
range_operation!(datastore_delete_by_btree_scan_bsatn, delete_index);

macro_rules! point_operation {
    ($name:ident, $method:ident) => {
        #[unsafe(no_mangle)]
        unsafe extern "C" fn $name(index: u32, point: *const u8, len: usize, out: *mut u32) -> u16 {
            let result = with_storage(|storage| {
                let rows = storage.point(index, unsafe { bytes(point, len) })?;
                storage.$method(index, rows)
            });
            unsafe { output(out, result) }
        }
    };
}

point_operation!(datastore_index_scan_point_bsatn, scan_index);
point_operation!(datastore_delete_by_index_scan_point_bsatn, delete_index);

#[unsafe(no_mangle)]
unsafe extern "C" fn row_iter_bsatn_advance(iter: u32, buffer: *mut u8, len: *mut usize) -> i16 {
    let capacity = unsafe { *len };
    let result = with_storage(|storage| {
        let rows = storage
            .iterators
            .get_mut(&iter)
            .ok_or(errno::NO_SUCH_ITER.get())?;
        let mut written = 0;
        while let Some(row) = rows.front() {
            if row.len() > capacity - written {
                if written == 0 {
                    unsafe {
                        len.write(row.len());
                    }
                    return Err(errno::BUFFER_TOO_SMALL.get());
                }
                break;
            }
            unsafe {
                ptr::copy_nonoverlapping(row.as_ptr(), buffer.add(written), row.len());
            }
            written += row.len();
            rows.pop_front();
        }
        unsafe {
            len.write(written);
        }
        if rows.is_empty() {
            storage.iterators.remove(&iter);
            Ok(-1)
        } else {
            Ok(0)
        }
    });
    result.unwrap_or_else(|code| code as i16)
}

#[unsafe(no_mangle)]
extern "C" fn row_iter_bsatn_close(iter: u32) -> u16 {
    with_storage(|storage| {
        storage
            .iterators
            .remove(&iter)
            .map(|_| 0)
            .ok_or(errno::NO_SUCH_ITER.get())
    })
    .unwrap_or_else(|code| code)
}

unsafe fn write_row(table: u32, index: Option<u32>, row: *mut u8, len: *mut usize) -> u16 {
    let capacity = unsafe { *len };
    let result =
        with_storage(|storage| storage.insert(table, unsafe { bytes(row, capacity) }, index));
    match result {
        Ok(generated) => {
            if generated.len() > capacity {
                return errno::BUFFER_TOO_SMALL.get();
            }
            unsafe {
                ptr::copy_nonoverlapping(generated.as_ptr(), row, generated.len());
                len.write(generated.len());
            }
            0
        }
        Err(code) => code,
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn datastore_insert_bsatn(table: u32, row: *mut u8, len: *mut usize) -> u16 {
    unsafe { write_row(table, None, row, len) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn datastore_update_bsatn(
    table: u32,
    index: u32,
    row: *mut u8,
    len: *mut usize,
) -> u16 {
    unsafe { write_row(table, Some(index), row, len) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn datastore_delete_all_by_eq_bsatn(
    table: u32,
    rows: *const u8,
    len: usize,
    out: *mut u32,
) -> u16 {
    unsafe {
        output(
            out,
            with_storage(|storage| storage.delete_equal(table, bytes(rows, len))),
        )
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn datastore_clear(table: u32, out: *mut u64) -> u16 {
    unsafe { output(out, with_storage(|storage| storage.clear(table))) }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn console_log(
    _level: u8,
    _target: *const u8,
    _target_len: usize,
    _filename: *const u8,
    _filename_len: usize,
    _line: u32,
    message: *const u8,
    len: usize,
) {
    eprintln!(
        "{}",
        String::from_utf8_lossy(unsafe { bytes(message, len) })
    );
}

#[unsafe(no_mangle)]
unsafe extern "C" fn identity(out: *mut u8) {
    unsafe {
        ptr::copy_nonoverlapping(
            spacetimedb::Identity::__dummy().to_byte_array().as_ptr(),
            out,
            32,
        );
    }
}

fn unsupported(name: &str) -> u16 {
    let _ = with_storage(|storage| {
        storage.fault = Some(format!("native host operation is not implemented: {name}"));
        Ok(())
    });
    errno::HOST_CALL_FAILURE.get()
}

#[unsafe(no_mangle)]
extern "C" fn console_timer_start(_name: *const u8, _len: usize) -> u32 {
    unsupported("console_timer_start").into()
}

#[unsafe(no_mangle)]
extern "C" fn console_timer_end(_id: u32) -> u16 {
    unsupported("console_timer_end")
}

#[unsafe(no_mangle)]
extern "C" fn volatile_nonatomic_schedule_immediate(
    _name: *const u8,
    _name_len: usize,
    _args: *const u8,
    _args_len: usize,
) {
    unsupported("volatile_nonatomic_schedule_immediate");
}

#[unsafe(no_mangle)]
extern "C" fn bytes_sink_write(_id: u32, _buffer: *const u8, _len: *mut usize) -> u16 {
    unsupported("bytes_sink_write")
}

#[unsafe(no_mangle)]
extern "C" fn bytes_source_read(_id: u32, _buffer: *mut u8, _len: *mut usize) -> i16 {
    unsupported("bytes_source_read") as i16
}

#[unsafe(no_mangle)]
extern "C" fn bytes_source_remaining_length(_id: u32, _out: *mut u32) -> i16 {
    unsupported("bytes_source_remaining_length") as i16
}

#[unsafe(no_mangle)]
extern "C" fn get_jwt(_connection: *const u8, _source: *mut u32) -> u16 {
    unsupported("get_jwt")
}

#[unsafe(no_mangle)]
extern "C" fn procedure_sleep_until(_at: i64) -> i64 {
    unsupported("procedure_sleep_until").into()
}

#[unsafe(no_mangle)]
extern "C" fn procedure_start_mut_tx(_at: *mut i64) -> u16 {
    unsupported("procedure_start_mut_tx")
}

#[unsafe(no_mangle)]
extern "C" fn procedure_commit_mut_tx() -> u16 {
    unsupported("procedure_commit_mut_tx")
}

#[unsafe(no_mangle)]
extern "C" fn procedure_abort_mut_tx() -> u16 {
    unsupported("procedure_abort_mut_tx")
}

#[unsafe(no_mangle)]
extern "C" fn procedure_http_request(
    _req: *const u8,
    _req_len: u32,
    _body: *const u8,
    _body_len: u32,
    _response: *mut [u32; 2],
) -> u16 {
    unsupported("procedure_http_request")
}
