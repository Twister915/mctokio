use anyhow::Result;
use anyhow::anyhow;
use aes::{
    Aes128,
    cipher::{
        generic_array::GenericArray,
        NewBlockCipher,
        BlockCipherMut,
        consts::U16,
    },
};

const BYTES_SIZE: usize = 16;

pub struct MinecraftCipher {
    iv: GenericArray<u8, U16>,
    tmp: GenericArray<u8, U16>,
    cipher: Aes128,
}

impl MinecraftCipher {
    pub fn new(key: &[u8], iv: &[u8]) -> Result<Self> {
        if iv.len() != BYTES_SIZE {
            return Err(anyhow!("iv needs to be 16 bytes"));
        }

        if key.len() != BYTES_SIZE {
            return Err(anyhow!("key needs to be 16 bytes"));
        }

        let mut iv_out = [0u8; BYTES_SIZE];
        iv_out.copy_from_slice(iv);

        let mut key_out = [0u8; BYTES_SIZE];
        key_out.copy_from_slice(key);

        let tmp = [0u8; BYTES_SIZE];

        Ok(Self {
            iv: GenericArray::from(iv_out),
            tmp: GenericArray::from(tmp),
            cipher: Aes128::new(&GenericArray::from(key_out)),
        })
    }

    pub fn encrypt(&mut self, data: &mut[u8]) {
        unsafe { self.crypt(data, false) }
    }

    pub fn decrypt(&mut self, data: &mut[u8]) {
        unsafe { self.crypt(data, true) }
    }

    unsafe fn crypt(&mut self, data: &mut[u8], decrypt: bool) {
        let iv = &mut self.iv;
        const IV_SIZE: usize = 16;
        const IV_SIZE_MINUS_ONE: usize = IV_SIZE - 1;
        let iv_ptr = iv.as_mut_ptr();
        let iv_end_ptr = iv_ptr.offset(IV_SIZE_MINUS_ONE as isize);
        let tmp_ptr = self.tmp.as_mut_ptr();
        let tmp_offset_one_ptr = tmp_ptr.offset(1);
        let cipher = &mut self.cipher;
        let n = data.len();
        let mut data_ptr = data.as_mut_ptr();
        let data_end_ptr = data_ptr.offset(n as isize);

        while data_ptr != data_end_ptr {
            std::ptr::copy_nonoverlapping(iv_ptr, tmp_ptr, IV_SIZE);
            cipher.encrypt_block(iv);
            let orig = *data_ptr;
            let updated = orig ^ *iv_ptr;
            std::ptr::copy_nonoverlapping(tmp_offset_one_ptr, iv_ptr, IV_SIZE_MINUS_ONE);
            if decrypt {
                *iv_end_ptr = orig;
            } else {
                *iv_end_ptr = updated;
            }
            *data_ptr = updated;
            data_ptr = data_ptr.offset(1);
        }
    }
}
