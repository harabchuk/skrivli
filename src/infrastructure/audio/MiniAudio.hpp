#pragma once

// These switches must match the MINIAUDIO_NO_* cache options in CMakeLists.txt.
// The static library and every translation unit that includes this header have
// to agree, or miniaudio's struct layouts will not match.
#define MA_NO_GENERATION
#define MA_NO_RESOURCE_MANAGER
#define MA_NO_NODE_GRAPH
#define MA_NO_ENGINE

#if defined(_MSC_VER)
#pragma warning(push, 0)
#endif
#include <miniaudio.h>
#if defined(_MSC_VER)
#pragma warning(pop)
#endif
