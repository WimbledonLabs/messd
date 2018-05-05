pub fn little_endian_to_int(bytes: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    let mut shift_value = 0;
    for b in bytes.iter() {
        sum += (*b as u32) << shift_value;
        shift_value += 8
    }
    sum
}

pub fn take_from_slice<'a>(bytes: &mut &[u8]) -> u8 {
    let (a, b) = bytes.split_first().unwrap();
    *bytes = b;
    *a
}

pub fn taken_from_slice<'a>(bytes: &'a mut &[u8], midpoint: usize) -> &'a[u8] {
    let (a, b) = bytes.split_at(midpoint);
    *bytes = b;
    a
}
