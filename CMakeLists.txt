cmake_minimum_required(VERSION 3.24)

project(MeetingBuddy LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

include(FetchContent)
# Official WTL 10.01 release (SourceForge). Do not track the GitHub mirror main branch.
FetchContent_Declare(
    wtl
    URL      https://downloads.sourceforge.net/project/wtl/WTL%2010/WTL%2010.01%20Release/WTL10_01_Release.zip
    URL_HASH SHA256=1A62EA728D088C7C5C7CFC76DB445E5C9F04923F92CADCF27B5D7678D85826A2
)
FetchContent_MakeAvailable(wtl)

add_executable(MeetingBuddy WIN32
    src/main.cpp
    src/Application.cpp
    src/ui/MainWindow.cpp
    src/ui/RecordView.cpp
    src/ui/RecordingsView.cpp
    src/ui/TrayIcon.cpp
    src/storage/Settings.cpp
    src/util/Paths.cpp
    src/util/Log.cpp
    src/util/HResult.cpp
    resources/MeetingBuddy.rc
)

target_include_directories(MeetingBuddy PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${CMAKE_CURRENT_SOURCE_DIR}/resources
    ${wtl_SOURCE_DIR}/Include
)

target_compile_definitions(MeetingBuddy PRIVATE
    UNICODE
    _UNICODE
    WIN32_LEAN_AND_MEAN
    _WTL_USE_CSTRING
    NOMINMAX
)

target_link_libraries(MeetingBuddy PRIVATE
    ole32
    uuid
    shell32
    shlwapi
    comctl32
    advapi32
)

if(MSVC)
    target_compile_options(MeetingBuddy PRIVATE /W4 /permissive- /utf-8)
    # Use main() with a WIN32 subsystem executable.
    # Manifest is embedded from resources/MeetingBuddy.rc — disable the linker default.
    target_link_options(MeetingBuddy PRIVATE /ENTRY:mainCRTStartup /MANIFEST:NO)
endif()
