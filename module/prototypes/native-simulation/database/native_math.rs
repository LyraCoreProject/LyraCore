macro_rules! unary {
    ($($name:ident: $ty:ty),* $(,)?) => {$(
        // Hidden ELF symbols bind this Module's calls without replacing the host's math functions.
        std::arch::global_asm!(concat!(".hidden ", stringify!($name)));
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(value: $ty) -> $ty {
            libm::$name(value)
        }
    )*};
}

macro_rules! binary {
    ($($name:ident: $ty:ty),* $(,)?) => {$(
        std::arch::global_asm!(concat!(".hidden ", stringify!($name)));
        #[unsafe(no_mangle)]
        pub extern "C" fn $name(left: $ty, right: $ty) -> $ty {
            libm::$name(left, right)
        }
    )*};
}

macro_rules! sincos {
    ($($name:ident: $ty:ty),* $(,)?) => {$(
        std::arch::global_asm!(concat!(".hidden ", stringify!($name)));
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(value: $ty, sine: *mut $ty, cosine: *mut $ty) {
            let (s, c) = libm::$name(value);
            // The compiler's sincos lowering supplies two writable outputs of the declared type.
            unsafe { sine.write(s); cosine.write(c); }
        }
    )*};
}

unary!(atan: f64, cos: f64, cosf: f32, log: f64, sin: f64, sinf: f32, tan: f64);
binary!(atan2: f64, atan2f: f32, hypotf: f32, pow: f64);
sincos!(sincos: f64, sincosf: f32);
