/// IEEE 754 binary16 -> binary32 conversion, implemented from the format
/// definition rather than pulled in as a dependency. This is the standard
/// bit-twiddling algorithm (handles zero, subnormals, normals, inf/NaN) —
/// not something specific to ggml. ggml itself precomputes a 65536-entry
/// lookup table for speed, but the *values* it produces are exactly this
/// conversion; a correct from-scratch implementation is bit-identical.
pub fn f16_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 0x1) as u32;
    let exp = ((bits >> 10) & 0x1F) as u32;
    let mant = (bits & 0x3FF) as u32;

    let f32_bits: u32 = if exp == 0 {
        if mant == 0 {
            sign << 31
        } else {
            // Subnormal half -> normalize into a normal f32: shift the
            // mantissa left until its implicit leading bit lands at bit 10,
            // counting shifts to derive the correct (negative) exponent.
            let mut e: i32 = -1;
            let mut m = mant;
            while m & 0x400 == 0 {
                e += 1;
                m <<= 1;
            }
            m &= 0x3FF;
            let exp32 = (127 - 15 - e) as u32;
            (sign << 31) | (exp32 << 23) | (m << 13)
        }
    } else if exp == 0x1F {
        // Inf or NaN.
        (sign << 31) | (0xFFu32 << 23) | (mant << 13)
    } else {
        let exp32 = (exp as i32 - 15 + 127) as u32;
        (sign << 31) | (exp32 << 23) | (mant << 13)
    };

    f32::from_bits(f32_bits)
}

#[cfg(test)]
mod tests {
    use super::f16_to_f32;

    #[test]
    fn known_values() {
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert_eq!(f16_to_f32(0x8000), -0.0);
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0xBC00), -1.0);
        assert_eq!(f16_to_f32(0x3800), 0.5);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(f16_to_f32(0x4200), 3.0);
        // smallest positive subnormal: 2^-24
        assert!((f16_to_f32(0x0001) - 2f32.powi(-24)).abs() < 1e-12);
        // largest subnormal: (1023/1024) * 2^-14
        assert!((f16_to_f32(0x03FF) - (1023.0 / 1024.0) * 2f32.powi(-14)).abs() < 1e-12);
        assert!(f16_to_f32(0x7C00).is_infinite());
        assert!(f16_to_f32(0x7C01).is_nan());
    }
}
