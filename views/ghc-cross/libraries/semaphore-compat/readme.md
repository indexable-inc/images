# semaphore-compat

`semaphore-compat` provides a cross-platform implementation of system semaphores
that abstracts over the `unix` and `Win32` libraries.

It supports:

  - Creating (`createSemaphore`, `freshSemaphore`), opening (`openSemaphore`)
    and closing (`destroySemaphore`) semaphores.
  - Waiting on a semaphore:
     - without blocking with `tryWaitOnSemaphore`,
     - blocking forever, with `waitOnSemaphore`,
     - blocking, in a separate thread and allowing interruption, with
       `forkWaitOnSemaphoreInterruptible` and `interruptWaitOnSemaphore`.
  - Releasing tokens to a semaphore (`releaseSemaphore`).
  - Querying the semaphore for its current value (`getSemaphoreValue`).
