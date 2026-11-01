cmake_minimum_required(VERSION 3.24)

project(MeetingBuddy LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

include(FetchContent)
FetchContent_Declare(
    wtl
    GIT_REPOSITORY https://github.com/Win32-WTL/WTL.git
    GIT_TAG        main
)
FetchContent_MakeAvailable(wtl)

add_executable(MeetingBuddy WIN32
    src/main.cpp
    src/Application.cpp
    src/MainWindow.cpp
)

target_include_directories(MeetingBuddy PRIVATE
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${wtl_SOURCE_DIR}/Include
)

target_compile_definitions(MeetingBuddy PRIVATE
    UNICODE
    _UNICODE
    WIN32_LEAN_AND_MEAN
    _WTL_USE_CSTRING
)

if(MSVC)
    target_compile_options(MeetingBuddy PRIVATE /W4)
    # Use main() with a WIN32 subsystem executable.
    target_link_options(MeetingBuddy PRIVATE /ENTRY:mainCRTStartup)
endif()
