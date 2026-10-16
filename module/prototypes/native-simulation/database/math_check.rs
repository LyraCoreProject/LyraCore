#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
mod native_math;

static mut RESULTS: [u64; 15] = [0; 15];

// The verifier calls this on one thread and copies the results before the next call.
#[unsafe(no_mangle)]
pub extern "C" fn evaluate(seed: u32) -> *const u64 {
    let next = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    let sign_x = if seed & 1 == 0 { 1.0 } else { -1.0 };
    let sign_y = if seed & 2 == 0 { 1.0 } else { -1.0 };
    let x = f64::from(seed) / f64::from(u32::MAX) * std::f64::consts::TAU * sign_x;
    let y = f64::from(next) / f64::from(u32::MAX) * std::f64::consts::TAU * sign_y;
    let xf = (seed as f32 / u32::MAX as f32) * std::f32::consts::TAU * sign_x as f32;
    let yf = (next as f32 / u32::MAX as f32) * std::f32::consts::TAU * sign_y as f32;
    let (s, c) = x.sin_cos();
    let (sf, cf) = xf.sin_cos();
    let values = [
        x.atan().to_bits(),
        x.atan2(y).to_bits(),
        u64::from(xf.atan2(yf).to_bits()),
        x.cos().to_bits(),
        u64::from(xf.cos().to_bits()),
        u64::from(xf.hypot(yf).to_bits()),
        (x.abs() + 0.01).ln().to_bits(),
        (x.abs() + 0.01).powf(y).to_bits(),
        x.sin().to_bits(),
        u64::from(xf.sin().to_bits()),
        x.tan().to_bits(),
        s.to_bits(),
        c.to_bits(),
        u64::from(sf.to_bits()),
        u64::from(cf.to_bits()),
    ];
    unsafe {
        RESULTS = values;
        (&raw const RESULTS).cast()
    }
}
