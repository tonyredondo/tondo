/* What corprof.h takes of the headers of Windows, as .NET's PAL gives it on
 * Linux: written for this test from what corprof.h uses, and no more. */
#ifndef TONDO_TEST_WINDOWS_H
#define TONDO_TEST_WINDOWS_H

#include <stddef.h>
#include <stdint.h>

/* The integer types of Windows, as their sizes are on Linux. */
typedef int32_t HRESULT;
typedef uint32_t ULONG, DWORD, UINT, UINT32, ULONG32;
typedef int32_t BOOL, LONG;
typedef uint8_t BYTE, UINT8, UCHAR;
typedef uint16_t USHORT, WCHAR;
typedef uint64_t UINT64, ULONG64;
typedef size_t SIZE_T;
typedef uintptr_t UINT_PTR, ULONG_PTR;
typedef void *HANDLE, *PVOID;
typedef WCHAR *LPWSTR;
typedef const WCHAR *LPCWSTR;

/* A GUID, and the names an interface's identity goes by. */
typedef struct _GUID {
    uint32_t Data1;
    uint16_t Data2;
    uint16_t Data3;
    uint8_t Data4[8];
} GUID;
typedef GUID IID, CLSID;
typedef const IID *REFIID;
typedef const GUID *REFGUID, *LPCGUID;
typedef const CLSID *REFCLSID;

/* What corprof.h takes of corhdr.h. */
typedef uint32_t mdToken, mdModule, mdTypeDef, mdMethodDef, mdFieldDef, mdSignature, mdTypeRef, mdMemberRef;
typedef int32_t CorElementType;
typedef struct ASSEMBLYMETADATA ASSEMBLYMETADATA;
typedef void *RPC_IF_HANDLE;

/* MIDL's words, which mean nothing to C on Linux. */
#define STDMETHODCALLTYPE
#define __stdcall
#define EXTERN_C extern
#define BEGIN_INTERFACE
#define END_INTERFACE
#define CONST_VTBL const
#define DECLSPEC_XFGVIRT(base, func)
#define DECLSPEC_UUID(x)
#define MIDL_INTERFACE(x) struct
#define interface struct
#define __RPC_FAR

/* The annotations of SAL, which say nothing to C. */
#define _COM_Outptr_
#define _In_
#define _Out_
#define _Inout_
#define _In_opt_
#define _Out_opt_
#define _In_reads_(n)
#define _In_reads_opt_(n)
#define _Out_writes_(n)
#define _Out_writes_to_(n, m)
#define _Out_writes_bytes_(n)
#define _Out_writes_bytes_to_(n, m)
#define _In_reads_bytes_(n)

#endif
