#include "sample.h"
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

struct sample_handle {
    int values[3];
};

sample_handle *sample_open(const char *name, sample_flags flags) {
    sample_handle *handle = malloc(sizeof *handle);
    handle->values[0] = (int)strlen(name);
    handle->values[1] = (int)flags;
    handle->values[2] = 7;
    return handle;
}

void sample_close(sample_handle *handle) { free(handle); }

int sample_sum(const int *values, size_t count) {
    int total = 0;
    for (size_t i = 0; i < count; i++) total += values[i];
    return total;
}

sample_point sample_mirror(sample_point point) {
    sample_point mirrored = {point.y, point.x};
    return mirrored;
}

int sample_visit_all(sample_handle *handle, sample_visit visit, void *context) {
    int total = 0;
    for (int i = 0; i < 3; i++) total += visit(context, handle->values[i]);
    return total;
}

int sample_format(char *buffer, size_t size, const char *format, ...) {
    va_list arguments;
    va_start(arguments, format);
    int written = vsnprintf(buffer, size, format, arguments);
    va_end(arguments);
    return written;
}

double sample_area(const struct sample_record *record) {
    return record->weights[0][0] * record->weights[1][1] + record->corner.x + record->nested.inner;
}

sample_color sample_next(sample_color color) { return color == SAMPLE_RED ? SAMPLE_GREEN : SAMPLE_BLUE; }

int type(int in) { return in + 1; }

int sample_bits_total(struct sample_bits bits) { return bits.a + bits.b + bits.delta + bits.on + (int)bits.scale; }

struct sample_bits sample_bits_make(unsigned a, unsigned b, int delta) {
    struct sample_bits bits = {2.0, a, b, delta, 1};
    return bits;
}

int sample_real(void) { return 99; }

sample_number sample_negate(sample_number number, int is_float) {
    if (is_float) {
        number.as_float = -number.as_float;
    } else {
        number.as_int = -number.as_int;
    }
    return number;
}

int sample_log(sample_logger logger, void *context, const char *format, ...) {
    va_list arguments;
    va_start(arguments, format);
    logger(context, format, arguments);
    va_end(arguments);
    return 0;
}

int sample_vformat(char *buffer, size_t size, const char *format, va_list arguments) {
    return vsnprintf(buffer, size, format, arguments);
}

int sample_is_all(sample_visit visit) { return visit == SAMPLE_ALL; }
