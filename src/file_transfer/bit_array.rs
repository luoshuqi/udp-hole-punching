use std::fmt::{Debug, Formatter};

/// bit 数组
pub struct BitArray {
    vec: Vec<u64>,
    len: u32,
}

impl Default for BitArray {
    fn default() -> Self {
        Self {
            vec: Vec::new(),
            len: 0,
        }
    }
}

impl BitArray {
    #[allow(dead_code)]
    pub fn new(len: u32) -> Self {
        let vec_len = len / 64 + (len % 64).min(1);
        let mut arr = Self {
            vec: vec![0; vec_len as usize],
            len,
        };
        arr.fill_unused();
        arr
    }

    pub fn reset(&mut self, len: u32) {
        let old_vec_len = self.vec.len();
        let new_vec_len = (len / 64 + (len % 64).min(1)) as usize;
        if new_vec_len > old_vec_len {
            self.vec.reserve(new_vec_len - old_vec_len);
        }
        unsafe {
            self.vec.set_len(new_vec_len);
            self.vec.as_mut_ptr().write_bytes(0, new_vec_len);
        };
        self.len = len;
        self.fill_unused();
    }

    pub fn is_set(&self, index: u32) -> bool {
        assert!(index < self.len, "{} < {}", index, self.len);

        let position = index / 64;
        let offset = index % 64;
        (self.vec[position as usize] & (1 << 63 - offset)) > 0
    }

    pub fn set(&mut self, index: u32) {
        assert!(index < self.len, "{} < {}", index, self.len);

        let position = index / 64;
        let offset = index % 64;
        self.vec[position as usize] |= 1 << 63 - offset
    }

    pub fn collect_unset(&self) -> Vec<u32> {
        let mut index = 0;
        let mut vec = Vec::new();
        for v in &self.vec {
            if *v != u64::MAX {
                for i in (0..=63).rev() {
                    if (*v & 1 << i) == 0 {
                        vec.push(index * 64 + 63 - i);
                    }
                }
            }
            index += 1;
        }
        vec
    }

    fn fill_unused(&mut self) {
        let unused = self.vec.len() * 64 - self.len as usize;
        if unused > 0 {
            let index = self.vec.len() - 1;
            self.vec[index] = u64::MAX >> (64 - unused);
        }
    }
}

impl Debug for BitArray {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            let len = self.vec.len();
            if len == 0 {
                f.write_str("")
            } else {
                for i in 0..len - 1 {
                    f.write_str(&format!("{:#066b}", self.vec[i]))?;
                    f.write_str(", ")?;
                }
                f.write_str(&format!("{:#066b}", self.vec[len - 1]))
            }
        } else {
            f.debug_struct("BitFlag")
                .field("flag", &self.vec)
                .field("count", &self.len)
                .finish()
        }
    }
}
