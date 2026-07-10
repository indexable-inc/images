//! virtio-net over PCI: the HAL glue `virtio-drivers` needs, a port-IO
//! configuration-space accessor, and NIC discovery.

use core::ptr::NonNull;

use virtio_drivers::device::net::VirtIONet;
use virtio_drivers::transport::pci::bus::{ConfigurationAccess, DeviceFunction, PciRoot};
use virtio_drivers::transport::pci::{PciTransport, virtio_device_type};
use virtio_drivers::transport::DeviceType;
use virtio_drivers::{BufferDirection, Hal, PhysAddr};
use x86_64::instructions::port::Port;

use crate::mem;
use crate::serial::println;

/// Queue depth for rx/tx virtqueues (QEMU's virtio-net default is 256; 16
/// is plenty for one polled HTTP connection at a time).
pub const QUEUE_SIZE: usize = 16;
/// Packet buffer size: an MTU-sized frame plus the virtio-net header.
pub const BUF_LEN: usize = 2048;

pub type Net = VirtIONet<HalImpl, PciTransport, QUEUE_SIZE>;

/// Finds the first virtio-net function on PCI bus 0 and brings it up.
///
/// The BIOS (SeaBIOS runs before our bootloader) has already assigned BARs,
/// so the transport only needs config-space access plus the MMIO mappings
/// `HalImpl` provides.
pub fn probe_net() -> Option<Net> {
    let mut root = PciRoot::new(PortCam);
    for (device_function, info) in root.enumerate_bus(0) {
        if virtio_device_type(&info) == Some(DeviceType::Network) {
            println!("virtio-net at {device_function} ({info})");
            let transport = PciTransport::new::<HalImpl, _>(&mut root, device_function)
                .inspect_err(|e| println!("virtio transport init failed: {e:?}"))
                .ok()?;
            let net = Net::new(transport, BUF_LEN)
                .inspect_err(|e| println!("virtio-net init failed: {e:?}"))
                .ok()?;
            return Some(net);
        }
    }
    None
}

/// Legacy x86 PCI Configuration Access Mechanism #1: config space through
/// I/O ports 0xCF8/0xCFC. Works on every QEMU machine type without needing
/// ACPI to find an ECAM window.
pub struct PortCam;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

fn config_address(df: DeviceFunction, register_offset: u8) -> u32 {
    0x8000_0000
        | u32::from(df.bus) << 16
        | u32::from(df.device) << 11
        | u32::from(df.function) << 8
        | u32::from(register_offset & 0xFC)
}

impl ConfigurationAccess for PortCam {
    fn read_word(&self, device_function: DeviceFunction, register_offset: u8) -> u32 {
        unsafe {
            Port::new(CONFIG_ADDRESS).write(config_address(device_function, register_offset));
            Port::new(CONFIG_DATA).read()
        }
    }

    fn write_word(&mut self, device_function: DeviceFunction, register_offset: u8, data: u32) {
        unsafe {
            Port::new(CONFIG_ADDRESS).write(config_address(device_function, register_offset));
            Port::new(CONFIG_DATA).write(data);
        }
    }

    unsafe fn unsafe_clone(&self) -> Self {
        PortCam
    }
}

pub struct HalImpl;

// SAFETY: dma_alloc returns zeroed, physically contiguous pages that are
// never handed out twice (the pool is a grow-only bump allocator), and the
// virt/phys translations are exact inverses of the bootloader's
// physical-memory offset mapping that all pool and heap memory lives in.
unsafe impl Hal for HalImpl {
    fn dma_alloc(pages: usize, _direction: BufferDirection) -> (PhysAddr, NonNull<u8>) {
        mem::with_phys(|phys| {
            let paddr = phys.alloc_pages(pages);
            (paddr, NonNull::new(phys.phys_to_virt(paddr)).unwrap())
        })
    }

    unsafe fn dma_dealloc(_paddr: PhysAddr, _vaddr: NonNull<u8>, _pages: usize) -> i32 {
        // Queues live for the lifetime of the machine; leaking is fine.
        0
    }

    unsafe fn mmio_phys_to_virt(paddr: PhysAddr, size: usize) -> NonNull<u8> {
        // PCI BARs sit above RAM, outside the bootloader's mapping; map them
        // (uncached) into the same offset window on first touch.
        mem::with_phys(|phys| NonNull::new(phys.map_mmio(paddr, size)).unwrap())
    }

    unsafe fn share(buffer: NonNull<[u8]>, _direction: BufferDirection) -> PhysAddr {
        // Driver buffers are heap allocations, and the heap lives inside the
        // offset-mapped carve, so translation is a subtraction.
        mem::with_phys(|phys| phys.virt_to_phys(buffer.as_ptr().cast::<u8>() as u64))
    }

    unsafe fn unshare(_paddr: PhysAddr, _buffer: NonNull<[u8]>, _direction: BufferDirection) {
        // Memory is always shared with the device; nothing to undo.
    }
}
