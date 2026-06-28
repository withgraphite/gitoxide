#include "system.h"

#ifdef _WIN32
struct gix_reftable_dir {
	HANDLE handle;
	WIN32_FIND_DATAW data;
	struct dirent entry;
	int first;
};

static wchar_t *gix_reftable_utf8_to_wide(const char *input)
{
	int len = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, input, -1,
				      NULL, 0);
	wchar_t *wide;

	if (!len) {
		errno = EINVAL;
		return NULL;
	}
	wide = malloc(sizeof(*wide) * len);
	if (!wide) {
		errno = ENOMEM;
		return NULL;
	}
	if (!MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, input, -1,
				 wide, len)) {
		free(wide);
		errno = EINVAL;
		return NULL;
	}
	return wide;
}

static void gix_reftable_set_errno(DWORD error)
{
	switch (error) {
	case ERROR_FILE_EXISTS:
	case ERROR_ALREADY_EXISTS:
		errno = EEXIST;
		break;
	case ERROR_FILE_NOT_FOUND:
	case ERROR_PATH_NOT_FOUND:
		errno = ENOENT;
		break;
	case ERROR_ACCESS_DENIED:
	case ERROR_SHARING_VIOLATION:
		errno = EACCES;
		break;
	case ERROR_DISK_FULL:
	case ERROR_HANDLE_DISK_FULL:
		errno = ENOSPC;
		break;
	case ERROR_INVALID_NAME:
		errno = EINVAL;
		break;
	default:
		errno = EIO;
		break;
	}
}

int gix_reftable_open_with_mode(const char *path, int flags, int mode)
{
	wchar_t *wide = gix_reftable_utf8_to_wide(path);
	DWORD access = GENERIC_READ;
	DWORD creation = OPEN_EXISTING;
	HANDLE handle;
	int fd;
	int descriptor_flags = flags & (_O_APPEND | _O_WRONLY | _O_RDWR |
					_O_TEXT | _O_BINARY | _O_NOINHERIT);
	(void)mode;
	if (!wide)
		return -1;
	if (flags & _O_RDWR)
		access = GENERIC_READ | GENERIC_WRITE;
	else if (flags & _O_WRONLY)
		access = GENERIC_WRITE;
	if (flags & _O_CREAT)
		creation = flags & _O_EXCL ? CREATE_NEW : OPEN_ALWAYS;
	handle = CreateFileW(wide, access,
			     FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
			     NULL, creation, FILE_ATTRIBUTE_NORMAL, NULL);
	free(wide);
	if (handle == INVALID_HANDLE_VALUE) {
		gix_reftable_set_errno(GetLastError());
		return -1;
	}
	fd = _open_osfhandle((intptr_t)handle, descriptor_flags);
	if (fd < 0)
		CloseHandle(handle);
	return fd;
}

int gix_reftable_open(const char *path, int flags)
{
	return gix_reftable_open_with_mode(path, flags, 0);
}

int gix_reftable_stat(const char *path, struct _stat64 *out)
{
	wchar_t *wide = gix_reftable_utf8_to_wide(path);
	int ret;

	if (!wide)
		return -1;
	ret = _wstat64(wide, out);
	free(wide);
	return ret;
}

int gix_reftable_unlink(const char *path)
{
	wchar_t *wide = gix_reftable_utf8_to_wide(path);
	int ret;

	if (!wide)
		return -1;
	ret = _wunlink(wide);
	free(wide);
	return ret;
}

int gix_reftable_rename(const char *from, const char *to)
{
	wchar_t *wide_from = gix_reftable_utf8_to_wide(from);
	wchar_t *wide_to;
	int ret;

	if (!wide_from)
		return -1;
	wide_to = gix_reftable_utf8_to_wide(to);
	if (!wide_to) {
		free(wide_from);
		return -1;
	}
	ret = MoveFileExW(wide_from, wide_to, MOVEFILE_REPLACE_EXISTING) ? 0 : -1;
	if (ret < 0)
		gix_reftable_set_errno(GetLastError());
	free(wide_to);
	free(wide_from);
	return ret;
}

int gix_reftable_chmod(const char *path, int mode)
{
	wchar_t *wide = gix_reftable_utf8_to_wide(path);
	int ret;

	if (!wide)
		return -1;
	ret = _wchmod(wide, mode);
	free(wide);
	return ret;
}

DIR *gix_reftable_opendir(const char *path)
{
	wchar_t *wide = gix_reftable_utf8_to_wide(path);
	wchar_t *pattern;
	struct gix_reftable_dir *dir;
	size_t len;

	if (!wide)
		return NULL;
	len = wcslen(wide);
	dir = calloc(1, sizeof(*dir));
	if (!dir) {
		free(wide);
		errno = ENOMEM;
		return NULL;
	}
	pattern = realloc(wide, sizeof(*wide) * (len + 3));
	if (!pattern) {
		free(wide);
		free(dir);
		errno = ENOMEM;
		return NULL;
	}
	wide = pattern;
	wide[len++] = L'\\';
	wide[len++] = L'*';
	wide[len] = L'\0';
	dir->handle = FindFirstFileW(wide, &dir->data);
	free(wide);
	if (dir->handle == INVALID_HANDLE_VALUE) {
		free(dir);
		errno = ENOENT;
		return NULL;
	}
	dir->first = 1;
	return dir;
}

struct dirent *gix_reftable_readdir(DIR *dir)
{
	int len;

	if (!dir->first && !FindNextFileW(dir->handle, &dir->data))
		return NULL;
	dir->first = 0;
	len = WideCharToMultiByte(CP_UTF8, 0, dir->data.cFileName, -1,
				  dir->entry.d_name, sizeof(dir->entry.d_name),
				  NULL, NULL);
	if (!len) {
		errno = EINVAL;
		return NULL;
	}
	return &dir->entry;
}

int gix_reftable_closedir(DIR *dir)
{
	int ret = FindClose(dir->handle) ? 0 : -1;
	free(dir);
	return ret;
}
#endif

int gix_reftable_sleep_ms(int timeout_ms)
{
#ifdef _WIN32
	if (timeout_ms > 0)
		Sleep((DWORD)timeout_ms);
	return 0;
#else
	struct timespec delay = {
		.tv_sec = timeout_ms / 1000,
		.tv_nsec = (timeout_ms % 1000) * 1000000L,
	};
	return nanosleep(&delay, NULL);
#endif
}
