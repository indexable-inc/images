//! Freestanding (`x86_64-unknown-none`) backend: no OS, no unikernel — this
//! binary *is* the kernel. The `bootloader` crate gets us into long mode with
//! all RAM mapped at an offset; from there we drive the virtio-net NIC
//! directly and run smoltcp as the TCP/IP stack, serving http-core responses
//! from a polled main loop.
//!
//! QEMU wiring assumed by the constants below (see xtask):
//! guest 10.0.2.15/24 behind user-mode NAT, gateway 10.0.2.2, host port
//! forwarded to guest 8080 via hostfwd.
#![no_std]
#![no_main]

extern crate alloc;

mod mem;
mod net;
mod serial;
mod virtio;

use alloc::vec::Vec;

use bootloader_api::config::Mapping;
use bootloader_api::{entry_point, BootInfo, BootloaderConfig};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, Ipv4Address};

use crate::serial::println;

const BACKEND: &str = "bare";
const PORT: u16 = 8080;
const IP: (u8, u8, u8, u8) = (10, 0, 2, 15);
const PREFIX_LEN: u8 = 24;
const GATEWAY: (u8, u8, u8, u8) = (10, 0, 2, 2);
/// Concurrent connections; each parked in LISTEN until accepted.
const SOCKET_COUNT: usize = 4;
const SOCKET_BUF: usize = 8192;

static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    // The whole memory model of mem.rs (and the virtio HAL's virt<->phys
    // translations) relies on this complete-physical-memory mapping.
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    serial::SERIAL.lock().init();
    println!("metal-httpd[{BACKEND}]: kernel booted");

    let phys_offset = boot_info
        .physical_memory_offset
        .into_option()
        .expect("bootloader did not map physical memory");
    mem::init(phys_offset, &boot_info.memory_regions);

    let nic = virtio::probe_net().expect("no virtio-net device on PCI bus 0");
    let mut device = net::NetDevice::new(nic);
    let mac = device.mac_address();
    println!(
        "MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    // Fixed seed: acceptable here because every boot serves throwaway test
    // traffic on an isolated user-mode network.
    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    config.random_seed = 0x6d65_74616c;
    let mut iface = Interface::new(config, &mut device, now());
    iface.update_ip_addrs(|addrs| {
        addrs
            .push(IpCidr::new(
                IpAddress::v4(IP.0, IP.1, IP.2, IP.3),
                PREFIX_LEN,
            ))
            .unwrap();
    });
    iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(GATEWAY.0, GATEWAY.1, GATEWAY.2, GATEWAY.3))
        .unwrap();

    let mut sockets = SocketSet::new(Vec::new());
    let mut conns: Vec<Conn> = (0..SOCKET_COUNT)
        .map(|_| {
            let rx = tcp::SocketBuffer::new(alloc::vec![0; SOCKET_BUF]);
            let tx = tcp::SocketBuffer::new(alloc::vec![0; SOCKET_BUF]);
            let mut socket = tcp::Socket::new(rx, tx);
            socket.listen(PORT).unwrap();
            Conn::new(sockets.add(socket))
        })
        .collect();

    println!(
        "metal-httpd[{BACKEND}]: listening on {}.{}.{}.{}:{PORT}",
        IP.0, IP.1, IP.2, IP.3
    );

    loop {
        let _ = iface.poll(now(), &mut device, &mut sockets);
        for conn in &mut conns {
            conn.service(sockets.get_mut::<tcp::Socket>(conn.handle));
        }
        core::hint::spin_loop();
    }
}

struct Conn {
    handle: SocketHandle,
    request: Vec<u8>,
    response: Option<Vec<u8>>,
    sent: usize,
}

impl Conn {
    fn new(handle: SocketHandle) -> Self {
        Self {
            handle,
            request: Vec::new(),
            response: None,
            sent: 0,
        }
    }

    /// One state-machine step: accumulate the request, render once complete,
    /// stream the response out, close, and re-arm the listener.
    fn service(&mut self, socket: &mut tcp::Socket) {
        if !socket.is_open() {
            self.request.clear();
            self.response = None;
            self.sent = 0;
            socket.listen(PORT).unwrap();
            return;
        }

        if self.response.is_none() {
            if socket.may_recv() {
                let complete = socket
                    .recv(|data| {
                        self.request.extend_from_slice(data);
                        (data.len(), http_core::request_complete(&self.request))
                    })
                    .unwrap_or(false);
                if complete {
                    let mut out = [0_u8; http_core::MAX_RESPONSE_LEN];
                    let len = http_core::render_response(&self.request, BACKEND, &mut out);
                    self.response = Some(out[..len].to_vec());
                }
            } else if socket.state() == tcp::State::CloseWait {
                // Peer gave up mid-request; nothing left to answer.
                socket.close();
            }
        }

        if let Some(response) = &self.response {
            if self.sent < response.len() && socket.may_send() {
                if let Ok(n) = socket.send_slice(&response[self.sent..]) {
                    self.sent += n;
                    if self.sent == response.len() {
                        socket.close();
                    }
                }
            }
        }
    }
}

/// Monotonic time from the TSC, assuming a 1 GHz clock. QEMU's TSC usually
/// runs faster, which only makes smoltcp's timers conservative — fine for a
/// test kernel that never leaves a virtual LAN.
fn now() -> Instant {
    let tsc = unsafe { core::arch::x86_64::_rdtsc() };
    Instant::from_micros((tsc / 1_000) as i64)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("KERNEL PANIC: {info}");
    exit_qemu(ExitCode::Failure);
}

#[derive(Clone, Copy)]
#[repr(u32)]
enum ExitCode {
    // QEMU exits with (code << 1) | 1, so this maps to exit status 3.
    Failure = 0x1,
}

/// Exits QEMU through the isa-debug-exit device the e2e harness attaches at
/// port 0xf4. Halts forever if the device is missing.
fn exit_qemu(code: ExitCode) -> ! {
    unsafe {
        x86_64::instructions::port::Port::new(0xf4).write(code as u32);
    }
    loop {
        x86_64::instructions::hlt();
    }
}
