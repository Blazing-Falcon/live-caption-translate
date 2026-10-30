pub fn to_byte(value: f64) -> u8 {
    if value.is_nan() || value <= -1.0 {
        return 0;
    }
    if value >= 255.5 {
        return 255;
    }
    (value.round_ties_even() as i32).clamp(0, 255) as u8
}
pub fn to_int(value: f64) -> i32 {
    value
        .round_ties_even()
        .clamp(i32::MIN as f64, i32::MAX as f64) as i32
}
pub fn to_short(value: f32) -> i16 {
    to_int(f64::from(value)).clamp(i16::MIN as i32, i16::MAX as i32) as i16
}
