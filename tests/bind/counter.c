/* A counter of counter.h made by C, with the table a program calls. */
#include <stdlib.h>
#include <string.h>
#include "counter.h"

static const GUID unknown = { 0x00000000, 0x0000, 0x0000, { 0xc0, 0, 0, 0, 0, 0, 0, 0x46 } };
static const GUID counting = { 0x2a0b2c6e, 0x5f1d, 0x4c3a, { 0x9b, 0x1e, 0x7d, 0x41, 0x0c, 0x55, 0x93, 0x21 } };
static const GUID naming = { 0x6c1e9f02, 0x3b7a, 0x4d5e, { 0x8f, 0x21, 0x0a, 0xb3, 0xc4, 0xd5, 0xe6, 0xf7 } };

typedef struct {
    INamedCounter object;
    ULONG count;
    int32_t total;
} Counter;

static HRESULT query(INamedCounter *self, REFIID id, void **found) {
    if (!memcmp(id, &unknown, sizeof(GUID)) || !memcmp(id, &counting, sizeof(GUID)) || !memcmp(id, &naming, sizeof(GUID))) {
        ((Counter *)self)->count++;
        *found = self;
        return 0;
    }
    *found = NULL;
    return (HRESULT)0x80004002;
}

static ULONG add_ref(INamedCounter *self) {
    return ++((Counter *)self)->count;
}

static ULONG release(INamedCounter *self) {
    Counter *counter = (Counter *)self;
    ULONG left = --counter->count;
    if (left == 0) {
        free(counter);
    }
    return left;
}

static HRESULT add(INamedCounter *self, int32_t amount) {
    if (amount < 0) {
        return (HRESULT)0x80070057;
    }
    ((Counter *)self)->total += amount;
    return 0;
}

static HRESULT get_total(INamedCounter *self, int32_t *total) {
    *total = ((Counter *)self)->total;
    return 0;
}

static HRESULT get_name(INamedCounter *self, ULONG capacity, ULONG *needed, WCHAR *name) {
    static const WCHAR text[] = { 't', 'a', 'l', 'l', 'y', 0 };
    (void)self;
    *needed = 5;
    if (capacity < 6) {
        return (HRESULT)0x8007007a;
    }
    memcpy(name, text, sizeof text);
    return 0;
}

static const INamedCounterVtbl table = { query, add_ref, release, add, get_total, get_name };

HRESULT counter_make(ICounter **made) {
    Counter *counter = calloc(1, sizeof *counter);
    counter->object.lpVtbl = &table;
    counter->count = 1;
    *made = (ICounter *)counter;
    return 0;
}
