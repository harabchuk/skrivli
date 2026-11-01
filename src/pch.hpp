#pragma once

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif

#ifndef NOMINMAX
#define NOMINMAX
#endif

#ifndef UNICODE
#define UNICODE
#endif

#ifndef _UNICODE
#define _UNICODE
#endif

#ifndef _WTL_USE_CSTRING
#define _WTL_USE_CSTRING
#endif

#include <atlbase.h>
#include <atlapp.h>
#include <atlwin.h>
#include <atlframe.h>
#include <atlctrls.h>
#include <atlctrlx.h>
#include <atldlgs.h>
#include <atlmisc.h>
#include <atlcrack.h>
#include <atlres.h>

#include <shellapi.h>
#include <shlwapi.h>
#include <shlobj.h>

extern WTL::CAppModule _Module;
