//! smoltcp `Device` implementation on top of the virtio-net driver.

use core::cell::RefCell;

use smoltcp::phy::{Checksum, Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::time::Instant;

use crate::virtio::{Net, BUF_LEN, QUEUE_SIZE};

/// `Device::receive` must hand out an rx and a tx token that both reach the
/// driver, so the driver sits behind a `RefCell`; the polled main loop is
/// single-threaded and tokens are consumed before the next poll.
pub struct NetDevice {
    net: RefCell<Net>,
}

impl NetDevice {
    pub fn new(net: Net) -> Self {
        Self {
            net: RefCell::new(net),
        }
    }

    pub fn mac_address(&self) -> [u8; 6] {
        self.net.borrow().mac_address()
    }
}

impl Device for NetDevice {
    type RxToken<'a> = VirtioRxToken<'a>;
    type TxToken<'a> = VirtioTxToken<'a>;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        let buffer = {
            let mut net = self.net.borrow_mut();
            if !net.can_recv() {
                return None;
            }
            net.receive().ok()?
        };
        Some((
            VirtioRxToken {
                net: &self.net,
                buffer,
            },
            VirtioTxToken { net: &self.net },
        ))
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        if !self.net.borrow().can_send() {
            return None;
        }
        Some(VirtioTxToken { net: &self.net })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = BUF_LEN - 12; // minus virtio-net header
        caps.max_burst_size = Some(QUEUE_SIZE);
        // virtio-drivers negotiates no checksum offload, so smoltcp computes
        // and verifies everything itself.
        caps.checksum.ipv4 = Checksum::Both;
        caps.checksum.tcp = Checksum::Both;
        caps
    }
}

pub struct VirtioRxToken<'a> {
    net: &'a RefCell<Net>,
    buffer: virtio_drivers::device::net::RxBuffer,
}

impl RxToken for VirtioRxToken<'_> {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        let result = f(self.buffer.packet());
        // Hand the buffer back to the rx virtqueue so the NIC can refill it.
        self.net
            .borrow_mut()
            .recycle_rx_buffer(self.buffer)
            .expect("rx buffer recycle failed");
        result
    }
}

pub struct VirtioTxToken<'a> {
    net: &'a RefCell<Net>,
}

impl TxToken for VirtioTxToken<'_> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut net = self.net.borrow_mut();
        let mut tx = net.new_tx_buffer(len);
        let result = f(tx.packet_mut());
        net.send(tx).expect("virtio send failed");
        result
    }
}
