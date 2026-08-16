---
name: io-uring
description: Linux io_uring async I/O. Use when working with io_uring in C (liburing) or Rust (tokio-rs/io-uring). Covers setup, submission/completion queues, operations, and best practices.
---

# io_uring

io_uring is Linux's high-performance async I/O interface (kernel 5.1+). Two ring buffers shared between kernel and userspace enable efficient batched syscalls.

## Core Concepts

- **SQ (Submission Queue)**: User submits I/O requests (SQEs)
- **CQ (Completion Queue)**: Kernel posts completions (CQEs)
- **SQE**: Submission Queue Entry - describes one I/O operation
- **CQE**: Completion Queue Entry - result of one operation

## C (liburing)

Reference: liburing source at https://github.com/axboe/liburing

### Basic Setup

```c
#include <liburing.h>

struct io_uring ring;
io_uring_queue_init(256, &ring, 0);  // 256 entries, no flags

// cleanup
io_uring_queue_exit(&ring);
```

### Submit and Wait Pattern

```c
struct io_uring_sqe *sqe = io_uring_get_sqe(&ring);
io_uring_prep_read(sqe, fd, buf, len, offset);
io_uring_sqe_set_data(sqe, user_data);  // attach context

io_uring_submit(&ring);  // submit to kernel

struct io_uring_cqe *cqe;
io_uring_wait_cqe(&ring, &cqe);  // block until completion

int result = cqe->res;  // bytes read or -errno
void *data = io_uring_cqe_get_data(cqe);
io_uring_cqe_seen(&ring, cqe);  // mark consumed
```

### Common Operations

```c
io_uring_prep_read(sqe, fd, buf, len, offset);
io_uring_prep_write(sqe, fd, buf, len, offset);
io_uring_prep_readv(sqe, fd, iovecs, nr_vecs, offset);
io_uring_prep_writev(sqe, fd, iovecs, nr_vecs, offset);
io_uring_prep_accept(sqe, sockfd, addr, addrlen, flags);
io_uring_prep_connect(sqe, sockfd, addr, addrlen);
io_uring_prep_send(sqe, sockfd, buf, len, flags);
io_uring_prep_recv(sqe, sockfd, buf, len, flags);
io_uring_prep_close(sqe, fd);
io_uring_prep_openat(sqe, dfd, path, flags, mode);
io_uring_prep_statx(sqe, dfd, path, flags, mask, statxbuf);
io_uring_prep_timeout(sqe, ts, count, flags);
io_uring_prep_poll_add(sqe, fd, poll_mask);
```

### Flags

```c
// Setup flags (io_uring_queue_init)
IORING_SETUP_SQPOLL    // kernel polls SQ, no submit syscalls needed
IORING_SETUP_IOPOLL    // busy-poll for completions (NVMe)

// SQE flags
IOSQE_FIXED_FILE       // use registered file index
IOSQE_IO_LINK          // link to next SQE (chain)
IOSQE_IO_DRAIN         // wait for prior ops to complete
IOSQE_ASYNC            // force async execution
```

### Registered Buffers/Files

```c
// Pre-register for zero-copy
struct iovec iovs[N];
io_uring_register_buffers(&ring, iovs, N);

int fds[M];
io_uring_register_files(&ring, fds, M);

// Use with IOSQE_FIXED_FILE and fixed buffer ops
io_uring_prep_read_fixed(sqe, fd_idx, buf, len, offset, buf_idx);
```

## Rust (tokio-rs/io-uring)

Reference: tokio-rs/io-uring source at https://github.com/tokio-rs/io-uring

### Setup

```rust
use io_uring::{IoUring, opcode, types};

let mut ring: IoUring = IoUring::new(256)?;
```

### Submit and Wait

```rust
let fd = types::Fd(file.as_raw_fd());
let read_e = opcode::Read::new(fd, buf.as_mut_ptr(), buf.len() as _)
    .offset(0)
    .build()
    .user_data(0x42);

unsafe {
    ring.submission().push(&read_e)?;
}

ring.submit_and_wait(1)?;

let cqe = ring.completion().next().unwrap();
let result = cqe.result();  // bytes or -errno
let user_data = cqe.user_data();
```

### Common Opcodes

```rust
opcode::Read::new(fd, buf, len).offset(off)
opcode::Write::new(fd, buf, len).offset(off)
opcode::Readv::new(fd, iovecs, nr_vecs)
opcode::Writev::new(fd, iovecs, nr_vecs)
opcode::Accept::new(fd, addr, addrlen)
opcode::Connect::new(fd, addr, addrlen)
opcode::Send::new(fd, buf, len)
opcode::Recv::new(fd, buf, len)
opcode::Close::new(fd)
opcode::OpenAt::new(fd, path)
opcode::Timeout::new(ts)
opcode::PollAdd::new(fd, mask)
```

### Squeue/Cqueue Access

```rust
let mut sq = ring.submission();
let mut cq = ring.completion();

// Check available slots
sq.is_full();
sq.capacity();
cq.is_empty();
cq.len();

// Sync with kernel
sq.sync();
cq.sync();
```

### Entry Builder Pattern

```rust
let entry = opcode::Read::new(fd, buf, len)
    .offset(offset)
    .build()
    .user_data(42)
    .flags(io_uring::squeue::Flags::ASYNC);
```

## Best Practices

1. **Batch operations** - submit multiple SQEs before calling submit()
2. **Use registered buffers/files** for hot paths (avoids per-op overhead)
3. **SQPOLL mode** for ultra-low latency (kernel thread polls SQ)
4. **Check kernel version** - features vary by kernel (5.1 base, 5.6+ for most ops)
5. **Handle EAGAIN** - SQ can fill up, CQ can overflow
6. **user_data for context** - associate requests with application state

## Kernel Version Features

- 5.1: Basic ops (read/write/fsync)
- 5.4: timeout, poll
- 5.5: accept, connect, send, recv
- 5.6: splice, provide_buffers, many improvements
- 5.7: link timeout, statx
- 5.11: shutdown, renameat, mkdirat
- 5.19: zero-copy send
- 6.0: zero-copy recv, multishot accept
