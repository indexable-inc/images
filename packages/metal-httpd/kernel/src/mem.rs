//! Physical + heap memory management.
//!
//! The bootloader maps *all physical RAM* at a fixed virtual offset
//! (`phys_offset`). We carve one usable physical region into:
//!
//! * a page pool for virtio DMA memory (virtqueue rings, and page-table
//!   frames for MMIO mappings), and
//! * the kernel heap.
//!
//! Placing the heap inside the offset-mapped region is deliberate: every
//! heap pointer translates to its physical address by subtracting
//! `phys_offset`, which is exactly what the virtio HAL's `share` needs when
//! the driver hands heap-allocated packet buffers to the device.

use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use linked_list_allocator::LockedHeap;
use spin::Mutex;
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::mapper::MapToError;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

pub const PAGE_SIZE: usize = 4096;
/// Physical pages reserved for DMA + page tables; the next `HEAP_SIZE` bytes
/// of the carved region become the heap.
const DMA_POOL_PAGES: usize = 512; // 2 MiB
const HEAP_SIZE: usize = 8 * 1024 * 1024;

#[global_allocator]
static HEAP: LockedHeap = LockedHeap::empty();

static PHYS: Mutex<Option<PhysMemory>> = Mutex::new(None);

pub struct PhysMemory {
    pool: PagePool,
    mapper: OffsetPageTable<'static>,
}

/// Bump allocator over the DMA pool; also feeds page-table frames to the
/// mapper. Nothing is ever freed: everything allocated here lives as long
/// as the machine runs.
struct PagePool {
    phys_offset: u64,
    next: u64,
    end: u64,
}

impl PagePool {
    /// Hands out zeroed, physically contiguous, page-aligned memory.
    fn alloc_pages(&mut self, pages: usize) -> u64 {
        let paddr = self.next;
        let end = paddr + (pages * PAGE_SIZE) as u64;
        assert!(end <= self.end, "DMA pool exhausted");
        self.next = end;
        unsafe {
            core::ptr::write_bytes((self.phys_offset + paddr) as *mut u8, 0, pages * PAGE_SIZE);
        }
        paddr
    }
}

unsafe impl FrameAllocator<Size4KiB> for PagePool {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        Some(PhysFrame::containing_address(PhysAddr::new(
            self.alloc_pages(1),
        )))
    }
}

impl PhysMemory {
    pub fn phys_to_virt(&self, paddr: u64) -> *mut u8 {
        (self.pool.phys_offset + paddr) as *mut u8
    }

    /// Translates a pointer into the offset-mapped region (heap or DMA pool)
    /// back to its physical address.
    pub fn virt_to_phys(&self, vaddr: u64) -> u64 {
        debug_assert!(vaddr >= self.pool.phys_offset, "address not offset-mapped");
        vaddr - self.pool.phys_offset
    }

    pub fn alloc_pages(&mut self, pages: usize) -> u64 {
        self.pool.alloc_pages(pages)
    }

    /// Maps physical MMIO space (PCI BARs live above RAM, so the
    /// bootloader's RAM mapping does not cover them) at the same
    /// `phys_offset` convention, uncached.
    pub fn map_mmio(&mut self, paddr: u64, size: usize) -> *mut u8 {
        let start = paddr & !(PAGE_SIZE as u64 - 1);
        let end = (paddr + size as u64).next_multiple_of(PAGE_SIZE as u64);
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_CACHE
            | PageTableFlags::NO_EXECUTE;
        for frame_addr in (start..end).step_by(PAGE_SIZE) {
            let page = Page::<Size4KiB>::containing_address(VirtAddr::new(
                self.pool.phys_offset + frame_addr,
            ));
            let frame = PhysFrame::containing_address(PhysAddr::new(frame_addr));
            // SAFETY: `frame` is device MMIO space claimed by nothing else in
            // this kernel, and the target page is inside the offset-mapped
            // window we own. Page-table frames come zeroed from the pool.
            match unsafe { self.mapper.map_to(page, frame, flags, &mut self.pool) } {
                Ok(flush) => flush.flush(),
                Err(MapToError::PageAlreadyMapped(_)) => {}
                // The bootloader's physical-memory mapping is built from huge
                // pages and (depending on RAM size rounding) can already
                // cover the BAR window. The linear virt=offset+phys relation
                // still holds, so the mapping is usable as-is. It is cached
                // (no PAT bits), which QEMU/TCG does not emulate anyway.
                Err(MapToError::ParentEntryHugePage) => {}
                Err(e) => panic!("MMIO map failed at {frame_addr:#x}: {e:?}"),
            }
        }
        self.phys_to_virt(paddr)
    }
}

/// Picks the largest usable RAM region, carves DMA pool + heap out of it,
/// and takes over the bootloader's page tables for later MMIO mappings.
pub fn init(phys_offset: u64, regions: &MemoryRegions) {
    let region = regions
        .iter()
        .filter(|r| r.kind == MemoryRegionKind::Usable && r.start >= 0x10_0000)
        .max_by_key(|r| r.end - r.start)
        .expect("no usable memory region");
    let needed = (DMA_POOL_PAGES * PAGE_SIZE + HEAP_SIZE) as u64;
    assert!(region.end - region.start >= needed, "largest region too small");

    // Page-align the carve in case the region is not.
    let start = region.start.next_multiple_of(PAGE_SIZE as u64);
    let dma_end = start + (DMA_POOL_PAGES * PAGE_SIZE) as u64;

    // SAFETY: Cr3 points at the bootloader-built level-4 table, which lives
    // in RAM and is therefore visible through the physical-memory offset.
    let mapper = unsafe {
        let (l4_frame, _) = Cr3::read();
        let l4: *mut PageTable = ((phys_offset + l4_frame.start_address().as_u64()) as *mut u8).cast();
        OffsetPageTable::new(&mut *l4, VirtAddr::new(phys_offset))
    };

    // SAFETY: the carved range is usable RAM the bootloader reports as free,
    // mapped at phys_offset, and nothing else allocates from it.
    unsafe {
        HEAP.lock().init((phys_offset + dma_end) as *mut u8, HEAP_SIZE);
    }

    *PHYS.lock() = Some(PhysMemory {
        pool: PagePool {
            phys_offset,
            next: start,
            end: dma_end,
        },
        mapper,
    });
}

/// Runs `f` with the global physical-memory manager. Panics before `init`.
pub fn with_phys<R>(f: impl FnOnce(&mut PhysMemory) -> R) -> R {
    f(PHYS.lock().as_mut().expect("mem::init not called"))
}
