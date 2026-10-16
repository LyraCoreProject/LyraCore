use std::cell::Cell;
use std::ffi::c_void;

include!("native_syscalls.rs");

#[path = "native_math.rs"]
mod native_math;

thread_local! {
    static ACTIVE: Cell<Option<(Dispatch, *mut c_void)>> = const { Cell::new(None) };
}

trait NativeResult {
    fn from_host(value: i64) -> Self;
}

impl NativeResult for () {
    fn from_host(_: i64) {}
}

macro_rules! integer_result {
    ($($ty:ty),*) => { $(
        impl NativeResult for $ty {
            fn from_host(value: i64) -> Self { value as Self }
        }
    )* };
}
integer_result!(u16, u32, i16, i64);

macro_rules! declare {
    ($($id:literal => $name:ident($($arg:ident: $ty:ty),*) -> $ret:ty;)*) => { $(
        #[unsafe(no_mangle)]
        unsafe extern "C" fn $name($($arg: $ty),*) -> $ret {
            let Some((dispatch, context)) = ACTIVE.get() else {
                std::process::abort();
            };
            let args = [$($arg as usize),*];
            // The host consumes these pointers synchronously before this SDK call returns.
            NativeResult::from_host(unsafe { dispatch(context, $id, args.as_ptr(), args.len()) })
        }
    )* };
}
native_syscalls!(declare);

// The catch stays inside the library, so a reducer panic cannot cross its C entry point.
#[unsafe(no_mangle)]
unsafe extern "C" fn __lyracore_native_v1(
    dispatch: Dispatch,
    context: *mut c_void,
    args: *const u64,
    describe: bool,
) -> i16 {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ACTIVE.set(None);
        }
    }
    if ACTIVE.get().is_some() {
        return -2;
    }
    ACTIVE.set(Some((dispatch, context)));
    let _reset = Reset;
    let result = std::panic::catch_unwind(|| {
        if describe {
            // The pinned SDK declares these opaque handles as repr(transparent) over u32.
            super::describe_module_inner(unsafe {
                std::mem::transmute::<u32, super::BytesSink>(1)
            });
            0
        } else {
            let args = unsafe { std::slice::from_raw_parts(args, 10) };
            super::call_reducer_inner(
                args[0] as usize,
                args[1],
                args[2],
                args[3],
                args[4],
                args[5],
                args[6],
                args[7],
                unsafe { std::mem::transmute::<u32, super::BytesSource>(args[8] as u32) },
                unsafe { std::mem::transmute::<u32, super::BytesSink>(args[9] as u32) },
            )
        }
    });
    result.unwrap_or(-2)
}
