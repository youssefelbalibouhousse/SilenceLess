/* config.h pour la compilation de LAME avec clang/NDK (cibles Android).
   Dérivé de configMS.h mais SANS les définitions de types propres à MSVC,
   qui entrent en conflit avec <stdint.h> fourni par le NDK. */
#ifndef CONFIG_H_INCLUDED
#define CONFIG_H_INCLUDED

/* Define if you have the ANSI C header files. */
#define STDC_HEADERS

/* Define if you have the <stdint.h> header file. */
#define HAVE_STDINT_H 1

/* Define if you have the <errno.h> header file. */
#define HAVE_ERRNO_H 1

/* Define if you have the <fcntl.h> header file. */
#define HAVE_FCNTL_H 1

/* Define if you have the <limits.h> header file. */
#define HAVE_LIMITS_H 1

/* Name of package */
#define PACKAGE "lame"

/* Define if compiler has function prototypes */
#define PROTOTYPES 1

/* faster log implementation with less but enough precision */
#define USE_FAST_LOG 1

#define HAVE_STRCHR 1
#define HAVE_MEMCPY 1

typedef float  float32_t;
typedef double float64_t;

typedef long double ieee854_float80_t;
typedef double      ieee754_float64_t;
typedef float       ieee754_float32_t;

#define LAME_LIBRARY_BUILD

#endif
