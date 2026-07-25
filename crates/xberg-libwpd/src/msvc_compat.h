/* Force-included (via `/FI`) ahead of every librevenge/libwpd translation unit
 * on MSVC, so the upstream sources stay byte-for-byte unmodified.
 *
 * librevenge 0.0.6 uses the POSIX `S_ISREG`/`S_ISDIR` macros in
 * RVNGDirectoryStream.cpp and RVNGStreamImplementation.cpp. MSVC's <sys/stat.h>
 * defines the `_S_IF*` mode bits but not these classification macros, so the
 * sources fail to compile with "identifier not found" there and only there. ~keep
 */
#ifndef XBERG_LIBWPD_MSVC_COMPAT_H
#define XBERG_LIBWPD_MSVC_COMPAT_H

#if defined(_MSC_VER)
#include <sys/stat.h>

#ifndef S_ISREG
#define S_ISREG(mode) (((mode) & _S_IFMT) == _S_IFREG)
#endif

#ifndef S_ISDIR
#define S_ISDIR(mode) (((mode) & _S_IFMT) == _S_IFDIR)
#endif
#endif /* _MSC_VER */

#endif /* XBERG_LIBWPD_MSVC_COMPAT_H */
