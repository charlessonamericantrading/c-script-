//! Skynet PagedAttention & Virtual Block KV Manager.
//!
//! Provides zero-fragmentation virtual memory allocation for KV Cache tensors
//! across multiple concurrent sequence streams, mirroring vLLM's PagedAttention.
//! 100% native Rust implementation with zero external dependencies.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// Physical KV block allocated in memory.
#[derive(Clone)]
pub struct PhysicalBlock {
    pub block_id: usize,
    pub block_size: usize, // e.g. 16 or 32 tokens per block
    pub ref_count: usize,
}

/// Maps logical token sequence positions to physical block IDs.
#[derive(Clone, Default)]
pub struct BlockTable {
    pub physical_block_ids: Vec<usize>,
}

/// Global Page Block Allocator for PagedAttention.
pub struct BlockManager {
    block_size: usize,
    num_total_blocks: usize,
    free_blocks: Mutex<VecDeque<usize>>,
    allocated: Mutex<HashMap<usize, PhysicalBlock>>,
}

impl BlockManager {
    /// Creates a new BlockManager with a total budget of physical blocks.
    pub fn new(num_total_blocks: usize, block_size: usize) -> Self {
        let mut free = VecDeque::with_capacity(num_total_blocks);
        for id in 0..num_total_blocks {
            free.push_back(id);
        }

        Self {
            block_size,
            num_total_blocks,
            free_blocks: Mutex::new(free),
            allocated: Mutex::new(HashMap::new()),
        }
    }

    /// Allocates a new physical block for a sequence stream.
    pub fn allocate_block(&self) -> Option<usize> {
        let mut free = self.free_blocks.lock().unwrap();
        let block_id = free.pop_front()?;

        let mut alloc = self.allocated.lock().unwrap();
        alloc.insert(
            block_id,
            PhysicalBlock {
                block_id,
                block_size: self.block_size,
                ref_count: 1,
            },
        );

        Some(block_id)
    }

    /// Increments reference count for shared prefix blocks (Prefix Cache sharing).
    pub fn share_block(&self, block_id: usize) {
        let mut alloc = self.allocated.lock().unwrap();
        if let Some(block) = alloc.get_mut(&block_id) {
            block.ref_count += 1;
        }
    }

    /// Frees a block or decrements reference count.
    pub fn free_block(&self, block_id: usize) {
        let mut alloc = self.allocated.lock().unwrap();
        let mut should_reclaim = false;

        if let Some(block) = alloc.get_mut(&block_id) {
            if block.ref_count > 1 {
                block.ref_count -= 1;
            } else {
                should_reclaim = true;
            }
        }

        if should_reclaim {
            alloc.remove(&block_id);
            let mut free = self.free_blocks.lock().unwrap();
            free.push_back(block_id);
        }
    }

    /// Returns the number of available free blocks.
    pub fn num_free_blocks(&self) -> usize {
        self.free_blocks.lock().unwrap().len()
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn num_total_blocks(&self) -> usize {
        self.num_total_blocks
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_manager_allocation_and_free() {
        let manager = BlockManager::new(10, 16);
        assert_eq!(manager.num_free_blocks(), 10);

        let b1 = manager.allocate_block().unwrap();
        let b2 = manager.allocate_block().unwrap();
        assert_eq!(manager.num_free_blocks(), 8);

        manager.share_block(b1);
        manager.free_block(b1); // ref_count drops to 1, not freed yet
        assert_eq!(manager.num_free_blocks(), 8);

        manager.free_block(b1); // ref_count drops to 0, reclaimed
        assert_eq!(manager.num_free_blocks(), 9);

        manager.free_block(b2);
        assert_eq!(manager.num_free_blocks(), 10);
    }
}
