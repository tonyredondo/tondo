/* unknwn.h of Windows: IUnknown, as an interface of C. */
#include "windows.h"

typedef struct IUnknown IUnknown;

typedef struct IUnknownVtbl {
    HRESULT (STDMETHODCALLTYPE *QueryInterface)(IUnknown *This, REFIID riid, void **ppvObject);
    ULONG (STDMETHODCALLTYPE *AddRef)(IUnknown *This);
    ULONG (STDMETHODCALLTYPE *Release)(IUnknown *This);
} IUnknownVtbl;

struct IUnknown {
    CONST_VTBL struct IUnknownVtbl *lpVtbl;
};
