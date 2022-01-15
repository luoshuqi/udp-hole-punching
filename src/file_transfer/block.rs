use std::cmp::Ordering::{Equal, Less};
use std::fs::{rename, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::slice::Chunks;

use crate::file_transfer::bit_array::BitArray;

/// 分块读文件
pub struct BlockReader {
    /// 文件
    file: File,
    /// block 分块大小
    block_size: u32,
    /// 最后一个 block 大小
    last_block_size: u32,
    /// chunk 分块大小
    chunk_size: u16,
    /// 下一个分块
    next_block: u32,
    /// 最后一个分块
    last_block: u32,
    /// block buffer
    buf: Vec<u8>,
}

impl BlockReader {
    pub fn new(
        mut file: File,
        file_size: u64,
        block_size: u32,
        chunk_size: u16,
        next_block: u32,
    ) -> crate::Result<Self> {
        if next_block > 0 {
            let offset = next_block as u64 * block_size as u64;
            file.seek(SeekFrom::Start(offset)).map_err(err!())?;
        }

        let (last_block, last_block_size) = last_block_index_size(file_size, block_size);
        let mut buf = Vec::with_capacity(block_size as usize);
        unsafe { buf.set_len(buf.capacity()) };

        Ok(Self {
            file,
            block_size,
            last_block_size,
            chunk_size,
            next_block,
            last_block,
            buf,
        })
    }

    /// 读取一个分块。返回 `None` 表示没有更多分块了
    pub fn read(&mut self) -> crate::Result<Option<Block>> {
        let len = match self.next_block.cmp(&self.last_block) {
            Less => self.block_size as usize,
            Equal => self.last_block_size as usize,
            _ => return Ok(None),
        };
        self.file.read_exact(&mut self.buf[..len]).map_err(err!())?;

        let block = Block {
            index: self.next_block,
            block: &self.buf[..len],
            chunk_size: self.chunk_size,
        };
        self.next_block += 1;
        Ok(Some(block))
    }
}

/// 最后一个分块的 index 和大小
fn last_block_index_size(file_size: u64, block_size: u32) -> (u32, u32) {
    let q = file_size / block_size as u64;
    let r = file_size % block_size as u64;
    if r == 0 {
        (q as u32 - 1, block_size)
    } else {
        (q as u32, r as u32)
    }
}

/// 读分块
pub struct Block<'a> {
    /// block index
    index: u32,
    /// block buffer
    block: &'a [u8],
    /// chunk 分块大小
    chunk_size: u16,
}

impl<'a> Block<'a> {
    pub fn index(&self) -> u32 {
        self.index
    }

    /// chunk 分块 iterator
    pub fn chunks(&self) -> Chunks<u8> {
        self.block.chunks(self.chunk_size as usize)
    }

    /// 获取 chunk 分块
    pub fn get_chunk(&self, index: u32) -> Option<&[u8]> {
        let start = self.chunk_size as usize * index as usize;
        if start < self.block.len() {
            let end = (start + self.chunk_size as usize).min(self.block.len());
            Some(&self.block[start..end])
        } else {
            None
        }
    }
}

/// 分块写文件
pub struct BlockWriter {
    /// 文件路径
    path: PathBuf,
    /// 文件
    file: File,
    /// block 分块大小
    block_size: u32,
    /// 最后一个 block 大小
    last_block_size: u32,
    /// chunk 分块大小
    chunk_size: u16,
    /// 下一个 block
    next_block: u32,
    /// 最后一个 block
    last_block: u32,
    /// block buffer
    buf: Vec<u8>,
    /// 记录 chunk 是否写入
    write_flag: BitArray,
}

impl BlockWriter {
    pub fn new(
        path: PathBuf,
        file_size: u64,
        block_size: u32,
        chunk_size: u16,
        resume: bool,
    ) -> crate::Result<Option<Self>> {
        if file_size == 0 {
            write_open(&path, false)?;
            return Ok(None);
        }

        let part = part_path(&path);
        let (file, next_block) = if resume && part.exists() {
            // 遇到同名文件会有问题，这里不考虑这种情况
            let mut file = write_open(&part, true)?;
            let size = file.metadata().map_err(err!())?.len();
            match size.cmp(&file_size) {
                Less => {
                    let next_block = size / block_size as u64;
                    let offset = next_block * block_size as u64;
                    file.seek(SeekFrom::Start(offset)).map_err(err!())?;
                    (file, next_block as u32)
                }
                Equal => {
                    rename_part_file(&part, &path)?;
                    return Ok(None);
                }
                _ => (write_open(&part, false)?, 0),
            }
        } else {
            (write_open(&part, false)?, 0)
        };

        let (last_block, last_block_size) = last_block_index_size(file_size, block_size);
        let mut buf = Vec::with_capacity(block_size as usize);
        unsafe { buf.set_len(buf.capacity()) };

        Ok(Some(Self {
            path,
            file,
            block_size,
            last_block_size,
            chunk_size,
            next_block,
            last_block,
            buf,
            write_flag: BitArray::default(),
        }))
    }

    pub fn next_block(&mut self) -> Option<BlockBuffer> {
        let block_size = match self.next_block.cmp(&self.last_block) {
            Less => self.block_size as usize,
            Equal => self.last_block_size as usize,
            _ => return None,
        } as u32;

        let chunk_count =
            block_size / self.chunk_size as u32 + 1.min(block_size % self.chunk_size as u32);
        self.write_flag.reset(chunk_count);

        let (last_chunk, last_chunk_size) = last_chunk_index_size(block_size, self.chunk_size);

        Some(BlockBuffer {
            writer: self,
            block_size,
            last_chunk,
            last_chunk_size,
        })
    }

    pub fn rename_file(&self) -> crate::Result<()> {
        let part = part_path(&self.path);
        rename_part_file(&part, &self.path)
    }
}

fn rename_part_file(part: &Path, name: &Path) -> crate::Result<()> {
    rename(&part, &name).map_err(err!("rename {} to {}", part.display(), name.display()))
}

fn part_path(path: &Path) -> PathBuf {
    match path.extension() {
        Some(ext) => path.with_extension(ext.to_str().unwrap().to_string() + ".part"),
        None => path.with_extension("part"),
    }
}

fn write_open(path: &Path, resume: bool) -> crate::Result<File> {
    OpenOptions::new()
        .create(!resume)
        .write(true)
        .truncate(!resume)
        .open(path)
        .map_err(err!("cannot open {}", path.display()))
}

/// 写分块
pub struct BlockBuffer<'a> {
    writer: &'a mut BlockWriter,
    block_size: u32,
    last_chunk: u32,
    last_chunk_size: u16,
}

impl<'a> BlockBuffer<'a> {
    pub fn index(&self) -> u32 {
        self.writer.next_block
    }

    /// 写入文件
    pub fn commit(&mut self) -> crate::Result<()> {
        self.writer.next_block += 1;
        self.writer
            .file
            .write_all(&self.writer.buf[..self.block_size as usize])
            .map_err(err!())
    }

    /// 写　chunk
    pub fn write(&mut self, chunk: u32, data: &[u8]) {
        match chunk.cmp(&self.last_chunk) {
            Less => assert_eq!(data.len(), self.writer.chunk_size as usize),
            Equal => assert_eq!(data.len(), self.last_chunk_size as usize),
            _ => panic!("chunk {} out of range", chunk),
        }
        if !self.writer.write_flag.is_set(chunk) {
            self.writer.write_flag.set(chunk);
            let start = self.writer.chunk_size as usize * chunk as usize;
            self.writer.buf[start..start + data.len()].copy_from_slice(data);
        }
    }

    /// 获取缺少的 chunk
    pub fn get_missing_chunk(&self) -> Vec<u32> {
        self.writer.write_flag.collect_unset()
    }
}

/// 最后一个分块的 index 和大小
fn last_chunk_index_size(block_size: u32, chunk_size: u16) -> (u32, u16) {
    let q = block_size / chunk_size as u32;
    let r = block_size % chunk_size as u32;
    if r == 0 {
        (q - 1, chunk_size)
    } else {
        (q, r as u16)
    }
}
