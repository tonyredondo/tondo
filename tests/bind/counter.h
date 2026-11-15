/* Interfaces as MIDL writes them, of C++ and of C: `tondo bind` reads the C
 * ones, with their IIDs from the C++ branch and from DEFINE_GUID. */
#ifndef COUNTER_H
#define COUNTER_H

#include <stdint.h>

typedef int32_t HRESULT;
typedef uint32_t ULONG;
typedef uint16_t WCHAR;
typedef struct _GUID {
    uint32_t Data1;
    uint16_t Data2;
    uint16_t Data3;
    unsigned char Data4[8];
} GUID;
typedef GUID IID;
typedef const IID *REFIID;

#define DEFINE_GUID(name, l, w1, w2, b1, b2, b3, b4, b5, b6, b7, b8) extern const GUID name

typedef struct ICounter ICounter;
typedef struct INamedCounter INamedCounter;

DEFINE_GUID(IID_INamedCounter, 0x6c1e9f02, 0x3b7a, 0x4d5e, 0x8f, 0x21, 0x0a, 0xb3, 0xc4, 0xd5, 0xe6, 0xf7);

#if defined(__cplusplus) && !defined(CINTERFACE)

MIDL_INTERFACE("2A0B2C6E-5F1D-4C3A-9B1E-7D410C559321")
ICounter : public IUnknown
{
public:
    virtual HRESULT STDMETHODCALLTYPE Add(int32_t amount) = 0;
    virtual HRESULT STDMETHODCALLTYPE GetTotal(int32_t *total) = 0;
};

MIDL_INTERFACE("6C1E9F02-3B7A-4D5E-8F21-0AB3C4D5E6F7")
INamedCounter : public ICounter
{
public:
    virtual HRESULT STDMETHODCALLTYPE GetName(ULONG capacity, ULONG *needed, WCHAR *name) = 0;
};

#else /* C style interface */

typedef struct ICounterVtbl {
    HRESULT (*QueryInterface)(ICounter *This, REFIID riid, void **ppvObject);
    ULONG (*AddRef)(ICounter *This);
    ULONG (*Release)(ICounter *This);
    HRESULT (*Add)(ICounter *This, int32_t amount);
    HRESULT (*GetTotal)(ICounter *This, int32_t *total);
} ICounterVtbl;

struct ICounter {
    const struct ICounterVtbl *lpVtbl;
};

typedef struct INamedCounterVtbl {
    HRESULT (*QueryInterface)(INamedCounter *This, REFIID riid, void **ppvObject);
    ULONG (*AddRef)(INamedCounter *This);
    ULONG (*Release)(INamedCounter *This);
    HRESULT (*Add)(INamedCounter *This, int32_t amount);
    HRESULT (*GetTotal)(INamedCounter *This, int32_t *total);
    HRESULT (*GetName)(INamedCounter *This, ULONG capacity, ULONG *needed, WCHAR *name);
} INamedCounterVtbl;

struct INamedCounter {
    const struct INamedCounterVtbl *lpVtbl;
};

#endif

/* A counter made by C, for a program to call. */
HRESULT counter_make(ICounter **made);

#endif
