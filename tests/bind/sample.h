/* A library for the tests of `tondo bind`. */
#ifndef SAMPLE_H
#define SAMPLE_H

#include <stdarg.h>
#include <stddef.h>

#define SAMPLE_VERSION "1.2"
#define SAMPLE_LIMIT 64
#define SAMPLE_MASK 0xFFu
#define SAMPLE_SHIFTED (1 << 4)
#define SAMPLE_BIG 5000000000
#define SAMPLE_RATIO 0.5
#define SAMPLE_NOTHING ((sample_visit)0)
#define SAMPLE_ALL ((sample_visit)-1)
#define SAMPLE_TWICE(x) ((x) * 2)
#define SAMPLE_OPEN(name) sample_open((name), SAMPLE_LIMIT)

typedef unsigned int sample_flags;
typedef struct sample_handle sample_handle;
typedef int (*sample_visit)(void *context, int value);

typedef enum { SAMPLE_RED, SAMPLE_GREEN = 5, SAMPLE_BLUE } sample_color;
enum sample_mode { SAMPLE_READ = 1, SAMPLE_WRITE = SAMPLE_READ << 1 };

typedef struct {
    int x;
    int y;
} sample_point;

struct sample_record {
    char name[16];
    double weights[2][2];
    sample_point corner;
    struct {
        int inner;
    } nested;
    union {
        int as_int;
        float as_float;
    } either;
    const char *label;
    sample_visit visit;
};

typedef union {
    int as_int;
    float as_float;
} sample_number;

typedef void (*sample_logger)(void *context, const char *format, va_list arguments);

struct sample_bits {
    unsigned a : 3;
    unsigned b : 5;
};

sample_handle *sample_open(const char *name, sample_flags flags);
void sample_close(sample_handle *handle);
int sample_sum(const int *values, size_t count);
sample_point sample_mirror(sample_point point);
int sample_visit_all(sample_handle *handle, sample_visit visit, void *context);
int sample_format(char *buffer, size_t size, const char *format, ...);
double sample_area(const struct sample_record *record);
sample_color sample_next(sample_color color);
int type(int in);
int sample_bits_total(struct sample_bits bits);
sample_number sample_negate(sample_number number, int is_float);
int sample_log(sample_logger logger, void *context, const char *format, ...);
int sample_vformat(char *buffer, size_t size, const char *format, va_list arguments);
int sample_is_all(sample_visit visit);
static inline int sample_inline(void) { return 1; }
#ifdef __APPLE__
int sample_renamed(void) __asm__("_sample_real");
#else
int sample_renamed(void) __asm__("sample_real");
#endif

#endif
