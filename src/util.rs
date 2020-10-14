pub fn get_sized_buf(raw_buf: &mut Vec<u8>, needed: usize) -> &mut [u8] {
    let cur_len = raw_buf.len();
    if cur_len < needed {
        let additional = needed - cur_len;
        raw_buf.reserve(additional);
        let new_len = raw_buf.capacity();
        unsafe {
            let start_at = raw_buf.as_mut_ptr();
            let new_start_at = start_at.offset(cur_len as isize);
            std::ptr::write_bytes(new_start_at, 0, new_len - cur_len);
            raw_buf.set_len(new_len);
        }
    }

    &mut raw_buf[..needed]
}

pub fn init_buf(buf: &mut Option<Vec<u8>>, default_cap: usize) -> &mut Vec<u8> {
    if let Some(buf) = buf {
        buf
    } else {
        let out = Vec::with_capacity(default_cap);
        *buf = Some(out);
        buf.as_mut().expect("just set")
    }
}