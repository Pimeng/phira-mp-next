//! IEEE-754 半精度浮点（float16）转换（对应文档 2.5 节 Float16 小节）。
//!
//! 注意点：
//! - 指数差必须用 i32 运算（u32 会下溢 panic）。
//! - 半→单 SNaN 需静默化；单→半 round-to-nearest-even。

const FP16_EXPONENT_BIAS: i32 = 15;
const FP32_EXPONENT_BIAS: i32 = 127;

/// 半精度 (u16 位模式) → 单精度 f32。
pub fn half_to_float(h: u16) -> f32 {
    f32::from_bits(half_to_float_bits(h))
}

fn half_to_float_bits(h: u16) -> u32 {
    let h = h as u32;
    let sign = (h & 0x8000) << 16;
    let e = ((h >> 10) & 0x1F) as i32; // 半精度指数（必须用 i32，避免下溢）
    let m = h & 0x03FF;

    let out = if e == 0 {
        if m == 0 {
            // ±0
            sign
        } else {
            // 次正规：归一化
            let mut e = e;
            let mut m = m;
            // 找到最高有效位
            while m & 0x0400 == 0 {
                m <<= 1;
                e -= 1;
            }
            e += 1;
            m &= !0x0400;
            let out_e = e + (FP32_EXPONENT_BIAS - FP16_EXPONENT_BIAS);
            sign | ((out_e as u32) << 23) | (m << 13)
        }
    } else if e == 0x1F {
        // Inf / NaN
        if m == 0 {
            sign | 0x7F80_0000
        } else {
            // NaN：静默化（置 quiet bit），保留尾数
            sign | 0x7F80_0000 | (m << 13) | 0x0040_0000
        }
    } else {
        let out_e = e + (FP32_EXPONENT_BIAS - FP16_EXPONENT_BIAS);
        sign | ((out_e as u32) << 23) | (m << 13)
    };
    out
}

/// 单精度 f32 → 半精度 (u16 位模式)，round-to-nearest-even。
pub fn float_to_half(f: f32) -> u16 {
    let bits = f.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let e = (((bits >> 23) & 0xFF) as i32) - FP32_EXPONENT_BIAS + FP16_EXPONENT_BIAS;
    let m = bits & 0x007F_FFFF;

    if e <= 0 {
        // 下溢 → 0 或次正规
        if e < -10 {
            return sign; // 太小 → ±0
        }
        // 转次正规：补隐含位，右移 (1 - e + 13) 位，带舍入
        let m = m | 0x0080_0000;
        let shift = (14 - e) as u32; // 14..=24
        let half_m = m >> shift;
        // round-to-nearest-even
        let round_bit = (m >> (shift - 1)) & 1;
        let sticky = if shift > 1 { (m & ((1 << (shift - 1)) - 1)) != 0 } else { false };
        let mut half_m = half_m;
        if round_bit == 1 && (sticky || (half_m & 1) == 1) {
            half_m += 1;
        }
        return sign | (half_m as u16);
    }
    if e >= 0x1F {
        // 溢出或 Inf/NaN
        if f.is_nan() {
            // 保留 NaN，静默化
            let half_m = (m >> 13) as u16;
            return sign | 0x7C00 | half_m | 0x0200;
        }
        return sign | 0x7C00; // ±Inf
    }
    // 正常范围：round-to-nearest-even
    let mut half_e = e as u16;
    let mut half_m = (m >> 13) as u16;
    let round_bits = m & 0x1FFF;
    if round_bits > 0x1000 || (round_bits == 0x1000 && (half_m & 1) == 1) {
        half_m += 1;
        if half_m == 0x0400 {
            half_m = 0;
            half_e += 1;
            if half_e >= 0x1F {
                return sign | 0x7C00; // 舍入溢出 → Inf
            }
        }
    }
    sign | (half_e << 10) | half_m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_values() {
        assert_eq!(half_to_float(0x0000), 0.0);
        assert_eq!(half_to_float(0x8000), -0.0);
        assert_eq!(half_to_float(0x3C00), 1.0);
        assert_eq!(half_to_float(0xC000), -2.0);
        assert_eq!(half_to_float(0x7C00), f32::INFINITY);
        assert_eq!(half_to_float(0xFC00), f32::NEG_INFINITY);
        assert!(half_to_float(0x7E00).is_nan());
        // 最大有限半精度 65504
        assert_eq!(half_to_float(0x7BFF), 65504.0);
        // 最小次正规 2^-24
        assert_eq!(half_to_float(0x0001), 5.9604645e-8);
    }

    #[test]
    fn roundtrip_common() {
        for v in [0.0f32, 1.0, -1.0, 0.5, 100.0, -100.5, 0.1, 65504.0] {
            let h = float_to_half(v);
            let back = half_to_float(h);
            let diff = (back - v).abs();
            assert!(diff <= v.abs() * 0.001 + 1e-6, "v={v} back={back}");
        }
        // 精确值
        assert_eq!(half_to_float(float_to_half(1.0)), 1.0);
        assert_eq!(float_to_half(f32::INFINITY), 0x7C00);
        assert_eq!(float_to_half(f32::NEG_INFINITY), 0xFC00);
        assert!(half_to_float(float_to_half(f32::NAN)).is_nan());
        assert_eq!(float_to_half(0.0), 0x0000);
    }

    #[test]
    fn subnormal_roundtrip() {
        let v = 5.9604645e-8f32; // 最小半精度次正规
        let h = float_to_half(v);
        assert_eq!(h, 0x0001);
        assert_eq!(half_to_float(h), v);
    }
}
