#include "system.h"
#include "basics.h"
#include "reftable-error.h"

#ifdef _WIN32
BOOLEAN NTAPI SystemFunction036(PVOID buffer, ULONG length);
#endif

#define REFTABLE_TEMP_ATTEMPTS 16384
#define REFTABLE_LOCK_SUFFIX ".lock"

struct reftable_platform_lock {
	char *lock_path;
	char *target_path;
};

static int random_bytes(void *buf, size_t len)
{
#ifdef _WIN32
	unsigned char *bytes = buf;

	while (len) {
		ULONG chunk = len > ULONG_MAX ? ULONG_MAX : (ULONG)len;
		if (!SystemFunction036(bytes, chunk)) {
			errno = EIO;
			return -1;
		}
		bytes += chunk;
		len -= chunk;
	}
	return 0;
#else
	unsigned char *bytes = buf;
	int fd;

	if (!len)
		return 0;
	fd = open("/dev/urandom", O_RDONLY);
	if (fd < 0)
		return -1;
	while (len) {
		ssize_t read_bytes = read(fd, bytes, len);
		if (read_bytes < 0) {
			int saved_errno = errno;
			if (saved_errno == EINTR)
				continue;
			close(fd);
			errno = saved_errno;
			return -1;
		}
		if (!read_bytes) {
			close(fd);
			errno = EIO;
			return -1;
		}
		bytes += read_bytes;
		len -= read_bytes;
	}
	close(fd);
	return 0;
#endif
}

uint32_t reftable_rand(void)
{
	uint32_t result;
	uint64_t fallback;

	if (!random_bytes(&result, sizeof(result)))
		return result;
	fallback = reftable_time_ms();
#ifdef _WIN32
	fallback ^= (uint64_t)GetCurrentProcessId() << 32;
	fallback ^= GetCurrentThreadId();
#else
	fallback ^= (uint64_t)getpid() << 32;
#endif
	fallback ^= (uintptr_t)&result;
	fallback ^= fallback >> 30;
	fallback *= UINT64_C(0xbf58476d1ce4e5b9);
	fallback ^= fallback >> 27;
	fallback *= UINT64_C(0x94d049bb133111eb);
	fallback ^= fallback >> 31;
	return (uint32_t)(fallback ^ (fallback >> 32));
}

static int create_file_exclusively(const char *path, int mode)
{
#ifdef _WIN32
	return gix_reftable_open_with_mode(path,
					  _O_RDWR | _O_CREAT | _O_EXCL |
						  _O_BINARY | _O_NOINHERIT,
					  mode);
#else
	return open(path, O_RDWR | O_CREAT | O_EXCL, mode);
#endif
}

static int close_file(int *fd)
{
	int open_fd;

	if (*fd < 0)
		return 0;
	open_fd = *fd;
	*fd = -1;
	return close(open_fd);
}

int tmpfile_from_pattern(struct reftable_tmpfile *out, const char *pattern)
{
	static const char letters[] =
		"abcdefghijklmnopqrstuvwxyz"
		"ABCDEFGHIJKLMNOPQRSTUVWXYZ"
		"0123456789";
	char *path;
	char *random_part;
	size_t len;
	int count;

	if (!out || !pattern)
		return REFTABLE_API_ERROR;
	len = strlen(pattern);
	if (len < 6 || memcmp(pattern + len - 6, "XXXXXX", 6))
		return REFTABLE_API_ERROR;
	path = reftable_strdup(pattern);
	if (!path)
		return REFTABLE_OUT_OF_MEMORY_ERROR;
	random_part = path + len - 6;

	for (count = 0; count < REFTABLE_TEMP_ATTEMPTS; count++) {
		uint64_t random;
		int fd;
		size_t i;

		if (random_bytes(&random, sizeof(random)) < 0) {
			reftable_free(path);
			return REFTABLE_IO_ERROR;
		}
		for (i = 0; i < 6; i++) {
			random_part[i] = letters[random % (sizeof(letters) - 1)];
			random /= sizeof(letters) - 1;
		}
		fd = create_file_exclusively(path, 0600);
		if (fd >= 0) {
			out->path = path;
			out->fd = fd;
			out->priv = path;
			return 0;
		}
		if (errno != EEXIST)
			break;
	}

	reftable_free(path);
	return REFTABLE_IO_ERROR;
}

int tmpfile_close(struct reftable_tmpfile *t)
{
	if (!t || !t->priv)
		return REFTABLE_API_ERROR;
	if (close_file(&t->fd) < 0)
		return REFTABLE_IO_ERROR;
	return 0;
}

int tmpfile_delete(struct reftable_tmpfile *t)
{
	char *path;
	int err = 0;

	if (!t)
		return REFTABLE_API_ERROR;
	if (!t->priv)
		return 0;
	path = t->priv;
	if (close_file(&t->fd) < 0)
		err = REFTABLE_IO_ERROR;
	if (unlink(path) < 0 && errno != ENOENT)
		err = REFTABLE_IO_ERROR;
	reftable_free(path);
	*t = REFTABLE_TMPFILE_INIT;
	return err;
}

int tmpfile_rename(struct reftable_tmpfile *t, const char *path)
{
	char *temp_path;
	int err = 0;

	if (!t || !t->priv || !path)
		return REFTABLE_API_ERROR;
	temp_path = t->priv;
	if (close_file(&t->fd) < 0 || rename(temp_path, path) < 0) {
		unlink(temp_path);
		err = REFTABLE_IO_ERROR;
	}
	reftable_free(temp_path);
	*t = REFTABLE_TMPFILE_INIT;
	return err;
}

static char *path_with_suffix(const char *path, const char *suffix)
{
	size_t path_len = strlen(path);
	size_t suffix_len = strlen(suffix);
	char *result;

	if (path_len > SIZE_MAX - suffix_len - 1) {
		errno = ENOMEM;
		return NULL;
	}
	result = reftable_malloc(path_len + suffix_len + 1);
	if (!result)
		return NULL;
	memcpy(result, path, path_len);
	memcpy(result + path_len, suffix, suffix_len + 1);
	return result;
}

static void free_lock(struct reftable_platform_lock *lock)
{
	reftable_free(lock->target_path);
	reftable_free(lock->lock_path);
	reftable_free(lock);
}

int flock_acquire(struct reftable_flock *l, const char *target_path,
		  long timeout_ms)
{
	struct reftable_platform_lock *lock;
	long remaining_ms = timeout_ms > 0 ? timeout_ms : 0;
	int n = 1;
	int multiplier = 1;

	if (!l || !target_path)
		return REFTABLE_API_ERROR;
	lock = reftable_calloc(1, sizeof(*lock));
	if (!lock)
		return REFTABLE_OUT_OF_MEMORY_ERROR;
	lock->lock_path = path_with_suffix(target_path, REFTABLE_LOCK_SUFFIX);
	lock->target_path = reftable_strdup(target_path);
	if (!lock->lock_path || !lock->target_path) {
		free_lock(lock);
		return REFTABLE_OUT_OF_MEMORY_ERROR;
	}

	for (;;) {
		int fd = create_file_exclusively(lock->lock_path, 0666);
		long backoff_ms;
		long wait_ms;

		if (fd >= 0) {
			l->path = lock->lock_path;
			l->fd = fd;
			l->priv = lock;
			return 0;
		}
		if (errno != EEXIST) {
			free_lock(lock);
			return REFTABLE_IO_ERROR;
		}
		if (!timeout_ms || (timeout_ms > 0 && remaining_ms <= 0)) {
			free_lock(lock);
			return REFTABLE_LOCK_ERROR;
		}

		backoff_ms = multiplier;
		wait_ms = (750 + reftable_rand() % 500) * backoff_ms / 1000;
		while (poll(NULL, 0, (int)wait_ms) < 0 && errno == EINTR)
			;
		if (timeout_ms > 0)
			remaining_ms -= wait_ms;
		multiplier += 2 * n + 1;
		if (multiplier > 1000)
			multiplier = 1000;
		else
			n++;
	}
}

int flock_close(struct reftable_flock *l)
{
	if (!l || !l->priv)
		return REFTABLE_API_ERROR;
	if (close_file(&l->fd) < 0)
		return REFTABLE_IO_ERROR;
	return 0;
}

int flock_release(struct reftable_flock *l)
{
	struct reftable_platform_lock *lock;
	int err = 0;

	if (!l)
		return REFTABLE_API_ERROR;
	if (!l->priv)
		return 0;
	lock = l->priv;
	if (close_file(&l->fd) < 0)
		err = REFTABLE_IO_ERROR;
	if (unlink(lock->lock_path) < 0 && errno != ENOENT)
		err = REFTABLE_IO_ERROR;
	free_lock(lock);
	*l = REFTABLE_FLOCK_INIT;
	return err;
}

int flock_commit(struct reftable_flock *l)
{
	struct reftable_platform_lock *lock;
	int err = 0;

	if (!l || !l->priv)
		return REFTABLE_API_ERROR;
	lock = l->priv;
	if (close_file(&l->fd) < 0 ||
	    rename(lock->lock_path, lock->target_path) < 0) {
		unlink(lock->lock_path);
		err = REFTABLE_IO_ERROR;
	}
	free_lock(lock);
	*l = REFTABLE_FLOCK_INIT;
	return err;
}

#undef fsync
int reftable_fsync(int fd)
{
#ifdef _WIN32
	return _commit(fd);
#else
	int ret;

	do {
		ret = fsync(fd);
	} while (ret < 0 && errno == EINTR);
	return ret;
#endif
}

uint64_t reftable_time_ms(void)
{
#ifdef _WIN32
	return GetTickCount64();
#else
	struct timespec now;

	if (!clock_gettime(CLOCK_MONOTONIC, &now))
		return (uint64_t)now.tv_sec * 1000 +
		       (uint64_t)now.tv_nsec / 1000000;
	return (uint64_t)time(NULL) * 1000;
#endif
}

int reftable_mmap(struct reftable_mmap *out, int fd, size_t len)
{
	if (!out)
		return REFTABLE_API_ERROR;
	if (!len)
		return REFTABLE_IO_ERROR;
#ifdef _WIN32
	intptr_t os_handle = _get_osfhandle(fd);
	HANDLE mapping;
	void *data;

	if (os_handle == -1)
		return REFTABLE_IO_ERROR;
	mapping = CreateFileMappingW((HANDLE)os_handle, NULL, PAGE_READONLY,
				     0, 0, NULL);
	if (!mapping)
		return REFTABLE_IO_ERROR;
	data = MapViewOfFile(mapping, FILE_MAP_READ, 0, 0, len);
	if (!data) {
		CloseHandle(mapping);
		return REFTABLE_IO_ERROR;
	}
	out->data = data;
	out->size = len;
	out->priv = mapping;
#else
	void *data = mmap(NULL, len, PROT_READ, MAP_PRIVATE, fd, 0);

	if (data == MAP_FAILED)
		return REFTABLE_IO_ERROR;
	out->data = data;
	out->size = len;
	out->priv = NULL;
#endif
	return 0;
}

int reftable_munmap(struct reftable_mmap *mapped)
{
	if (!mapped || !mapped->data)
		return REFTABLE_API_ERROR;
#ifdef _WIN32
	int err = 0;

	if (!UnmapViewOfFile(mapped->data))
		err = REFTABLE_IO_ERROR;
	if (mapped->priv && !CloseHandle(mapped->priv))
		err = REFTABLE_IO_ERROR;
	memset(mapped, 0, sizeof(*mapped));
	return err;
#else
	if (munmap(mapped->data, mapped->size) < 0)
		return REFTABLE_IO_ERROR;
	memset(mapped, 0, sizeof(*mapped));
	return 0;
#endif
}
