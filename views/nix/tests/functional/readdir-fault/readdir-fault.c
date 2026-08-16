/* Make `readdir` fail partway through one named directory.

   `readdir` reports end-of-directory and failure the same way, by returning
   NULL, and the two are told apart only by whether `errno` was set. That makes
   a swallowed error invisible by construction: the caller sees a short
   directory and no indication that it was short. Nothing available to a test
   makes a real directory read fail on demand -- permissions are checked at
   `opendir`, and EIO or ESTALE need hardware or a network filesystem
   misbehaving -- so interpose the libc entry points and fail there, which is
   the same thing the caller sees.

   Scoped by directory: `opendir` records the path behind each `DIR *`, and
   only reads of `NIX_READDIR_FAULT_DIR` fail, after
   `NIX_READDIR_FAULT_AFTER` successful entries. Everything else, including
   every other directory this process reads, is passed straight through. With
   no `NIX_READDIR_FAULT_DIR` set the library is inert, which is what lets the
   test prove the failure comes from the fault and not from the preloading. */

/* The build passes -D_FILE_OFFSET_BITS=64, under which glibc redirects
   `readdir` to `readdir64` and the two definitions below collide at assembly
   time. Interposition needs both symbols under their own names, so turn the
   redirection off; nothing here dereferences a `struct dirent`. */
#undef _FILE_OFFSET_BITS

#define _GNU_SOURCE

#include <dirent.h>
#include <dlfcn.h>
#include <errno.h>
#include <limits.h>
#include <stdlib.h>
#include <string.h>

#define MAX_TRACKED 64

static DIR * tracked_dir[MAX_TRACKED];
static int tracked_count[MAX_TRACKED];
static int tracked_n = 0;

static const char * fault_dir(void)
{
    return getenv("NIX_READDIR_FAULT_DIR");
}

static int fault_after(void)
{
    const char * s = getenv("NIX_READDIR_FAULT_AFTER");
    return s ? atoi(s) : 0;
}

static int fault_errno(void)
{
    const char * s = getenv("NIX_READDIR_FAULT_ERRNO");
    return s ? atoi(s) : EIO;
}

DIR * opendir(const char * name)
{
    static DIR * (*real)(const char *) = NULL;
    if (!real)
        real = dlsym(RTLD_NEXT, "opendir");

    DIR * dir = real(name);

    const char * target = fault_dir();
    if (dir && target && strcmp(name, target) == 0 && tracked_n < MAX_TRACKED) {
        tracked_dir[tracked_n] = dir;
        tracked_count[tracked_n] = 0;
        tracked_n++;
    }

    return dir;
}

/* Returns the slot for a tracked `DIR *`, or -1. */
static int slot_of(DIR * dir)
{
    for (int i = 0; i < tracked_n; i++)
        if (tracked_dir[i] == dir)
            return i;
    return -1;
}

static int should_fail(DIR * dir)
{
    int i = slot_of(dir);
    if (i < 0)
        return 0;
    if (tracked_count[i] >= fault_after())
        return 1;
    tracked_count[i]++;
    return 0;
}

struct dirent * readdir(DIR * dir)
{
    static struct dirent * (*real)(DIR *) = NULL;
    if (!real)
        real = dlsym(RTLD_NEXT, "readdir");

    if (should_fail(dir)) {
        errno = fault_errno();
        return NULL;
    }

    return real(dir);
}

/* Which of the two a caller compiled against depends on `_FILE_OFFSET_BITS`,
   so both have to be here or the interposition silently does nothing. */
struct dirent64 * readdir64(DIR * dir)
{
    static struct dirent64 * (*real)(DIR *) = NULL;
    if (!real)
        real = dlsym(RTLD_NEXT, "readdir64");

    if (should_fail(dir)) {
        errno = fault_errno();
        return NULL;
    }

    return real(dir);
}
