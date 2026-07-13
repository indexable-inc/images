//! COM1 serial output. The only console a headless QEMU guest has; the e2e
//! harness watches this for boot progress and panic reports.

use core::fmt::{self, Write};

use spin::Mutex;
use x86_64::instructions::port::Port;

const COM1: u16 = 0x3F8;

pub static SERIAL: Mutex<SerialPort> = Mutex::new(SerialPort { base: COM1 });

pub struct SerialPort {
    base: u16,
}

impl SerialPort {
    /// Standard 16550 init: 115200 8N1, FIFOs on, no interrupts (the kernel
    /// runs fully polled).
    pub fn init(&mut self) {
        unsafe {
            Port::<u8>::new(self.base + 1).write(0x00); // disable interrupts
            Port::<u8>::new(self.base + 3).write(0x80); // DLAB on
            Port::<u8>::new(self.base).write(0x01); // divisor 1 = 115200 baud
            Port::<u8>::new(self.base + 1).write(0x00);
            Port::<u8>::new(self.base + 3).write(0x03); // 8N1, DLAB off
            Port::<u8>::new(self.base + 2).write(0xC7); // FIFO on, clear
        }
    }

    fn write_byte(&mut self, byte: u8) {
        unsafe {
            let mut lsr = Port::<u8>::new(self.base + 5);
            while lsr.read() & 0x20 == 0 {
                core::hint::spin_loop();
            }
            Port::<u8>::new(self.base).write(byte);
        }
    }
}

impl Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}

pub fn print(args: fmt::Arguments<'_>) {
    let _ = SERIAL.lock().write_fmt(args);
}

macro_rules! println {
    ($($arg:tt)*) => {
        $crate::serial::print(format_args!("{}\n", format_args!($($arg)*)))
    };
}
pub(crate) use println;
