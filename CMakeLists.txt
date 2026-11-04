cmake_minimum_required(VERSION 3.24)

project(MeetingBuddy LANGUAGES C CXX)

set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

option(MEETINGBUDDY_BUILD_TESTS "Build unit tests" ON)

include(FetchContent)
# Official WTL 10.01 release (SourceForge). Do not track the GitHub mirror main branch.
FetchContent_Declare(
    wtl
    URL      https://downloads.sourceforge.net/project/wtl/WTL%2010/WTL%2010.01%20Release/WTL10_01_Release.zip
    URL_HASH SHA256=1A62EA728D088C7C5C7CFC76DB445E5C9F04923F92CADCF27B5D7678D85826A2
)
FetchContent_MakeAvailable(wtl)

# Glaze JSON (header-only). Requires C++23 and MSVC /Zc:preprocessor.
FetchContent_Declare(
    glaze
    GIT_REPOSITORY https://github.com/stephenberry/glaze.git
    GIT_TAG        v9.1.0
    GIT_SHALLOW    TRUE
)
FetchContent_MakeAvailable(glaze)

# whisper.cpp v1.9.5 — CPU only, static, portable AVX2 (no native host tuning).
set(WHISPER_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(WHISPER_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(WHISPER_BUILD_SERVER OFF CACHE BOOL "" FORCE)
set(BUILD_SHARED_LIBS OFF CACHE BOOL "" FORCE)
set(GGML_NATIVE OFF CACHE BOOL "" FORCE)
set(GGML_AVX ON CACHE BOOL "" FORCE)
set(GGML_AVX2 ON CACHE BOOL "" FORCE)
set(GGML_FMA ON CACHE BOOL "" FORCE)
set(GGML_F16C ON CACHE BOOL "" FORCE)
set(GGML_CUDA OFF CACHE BOOL "" FORCE)
set(GGML_VULKAN OFF CACHE BOOL "" FORCE)
set(GGML_OPENMP OFF CACHE BOOL "" FORCE)
set(GGML_BLAS OFF CACHE BOOL "" FORCE)
FetchContent_Declare(
    whisper
    GIT_REPOSITORY https://github.com/ggml-org/whisper.cpp.git
    GIT_TAG        v1.9.5
    GIT_SHALLOW    TRUE
)
FetchContent_MakeAvailable(whisper)

# miniaudio 0.11.22 — WAV encode/decode, resampling, channel conversion, and mixing.
# Device I/O stays on WASAPI in this app. Keep these options in sync with src/infrastructure/audio/MiniAudio.hpp.
set(MINIAUDIO_BUILD_EXAMPLES OFF CACHE BOOL "" FORCE)
set(MINIAUDIO_BUILD_TESTS OFF CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_EXTRA_NODES ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_LIBVORBIS ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_LIBOPUS ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_DEVICEIO ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_THREADING ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_GENERATION ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_RESOURCE_MANAGER ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_NODE_GRAPH ON CACHE BOOL "" FORCE)
set(MINIAUDIO_NO_ENGINE ON CACHE BOOL "" FORCE)
FetchContent_Declare(
    miniaudio
    GIT_REPOSITORY https://github.com/mackron/miniaudio.git
    GIT_TAG        0.11.22
    GIT_SHALLOW    TRUE
)
FetchContent_MakeAvailable(miniaudio)

# WIL v1.0.260126.7 — header-only. Do not add_subdirectory: upstream lists require
# CMake 3.29 and build tests and packaging by default.
FetchContent_Declare(
    wil
    GIT_REPOSITORY https://github.com/microsoft/wil.git
    GIT_TAG        v1.0.260126.7
    GIT_SHALLOW    TRUE
)
FetchContent_GetProperties(wil)
if(NOT wil_POPULATED)
    FetchContent_Populate(wil)
endif()
add_library(WIL_WIL INTERFACE)
add_library(WIL::WIL ALIAS WIL_WIL)
target_include_directories(WIL_WIL SYSTEM INTERFACE "${wil_SOURCE_DIR}/include")

set(MEETINGBUDDY_CORE_SOURCES
    src/core/TranscriptText.cpp
)

set(MEETINGBUDDY_SERVICES_SOURCES
    src/services/RecordingSession.cpp
    src/services/LevelMonitor.cpp
    src/services/TranscriptionService.cpp
)

set(MEETINGBUDDY_INFRA_SOURCES
    src/infrastructure/audio/DeviceManager.cpp
    src/infrastructure/audio/CaptureStream.cpp
    src/infrastructure/audio/AudioResampler.cpp
    src/infrastructure/audio/MaConverter.cpp
    src/infrastructure/audio/SilentRenderKeepAlive.cpp
    src/infrastructure/audio/Mixer.cpp
    src/infrastructure/audio/WavHeader.cpp
    src/infrastructure/audio/Resampler.cpp
    src/infrastructure/ai/WhisperEngine.cpp
    src/infrastructure/ai/ModelManager.cpp
    src/infrastructure/storage/Settings.cpp
    src/infrastructure/storage/RecordingSidecar.cpp
    src/infrastructure/storage/RecordingLibrary.cpp
    src/infrastructure/storage/DirectoryWatcher.cpp
    src/infrastructure/files/WavWriter.cpp
    src/infrastructure/files/M4aEncoder.cpp
    src/infrastructure/files/AudioFileReader.cpp
    src/infrastructure/files/AudioPlayer.cpp
    src/infrastructure/system/Autostart.cpp
    src/infrastructure/system/Hotkey.cpp
    src/infrastructure/system/Clipboard.cpp
    src/infrastructure/common/Paths.cpp
    src/infrastructure/common/Text.cpp
    src/infrastructure/common/FileIo.cpp
    src/infrastructure/common/Log.cpp
    src/infrastructure/common/HResult.cpp
)

set(MEETINGBUDDY_UI_SOURCES
    src/ui/DeviceCombo.cpp
    src/ui/ShellDialogs.cpp
    src/ui/TabCtrl.cpp
    src/ui/TrayIcon.cpp
    src/ui/LevelMeter.cpp
    src/ui/RecordButton.cpp
)

set(MEETINGBUDDY_VIEWS_SOURCES
    src/views/MainWindow.cpp
    src/views/RecordView.cpp
    src/views/RecordingsView.cpp
    src/views/SettingsDialog.cpp
    src/views/ModelPrompt.cpp
)

add_executable(MeetingBuddy WIN32
    src/main.cpp
    src/app/Application.cpp
    ${MEETINGBUDDY_CORE_SOURCES}
    ${MEETINGBUDDY_SERVICES_SOURCES}
    ${MEETINGBUDDY_INFRA_SOURCES}
    ${MEETINGBUDDY_UI_SOURCES}
    ${MEETINGBUDDY_VIEWS_SOURCES}
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
    WIL_SUPPRESS_EXCEPTIONS
)

target_link_libraries(MeetingBuddy PRIVATE
    WIL::WIL
    whisper
    miniaudio
    glaze::glaze
    ole32
    uuid
    shell32
    shlwapi
    comctl32
    advapi32
    avrt
    propsys
    winmm
    mfplat
    mf
    mfplay
    mfreadwrite
    mfuuid
    wmcodecdspuuid
    winhttp
    bcrypt
    crypt32
    uxtheme
)

if(MSVC)
    target_compile_options(MeetingBuddy PRIVATE /W4 /permissive- /utf-8 /Zc:preprocessor)
    # Use main() with a WIN32 subsystem executable.
    # Manifest is embedded from resources/MeetingBuddy.rc — disable the linker default.
    target_link_options(MeetingBuddy PRIVATE /ENTRY:mainCRTStartup /MANIFEST:NO)
endif()

if(MEETINGBUDDY_BUILD_TESTS)
    set(gtest_force_shared_crt ON CACHE BOOL "" FORCE)
    FetchContent_Declare(
        googletest
        GIT_REPOSITORY https://github.com/google/googletest.git
        GIT_TAG        v1.15.2
    )
    FetchContent_MakeAvailable(googletest)

    enable_testing()

    add_executable(MeetingBuddyTests
        src/core/tests/RingBufferTests.cpp
        src/core/tests/ChunkPlanTests.cpp
        src/core/tests/TranscriptTextTests.cpp
        src/infrastructure/audio/tests/MixerTests.cpp
        src/infrastructure/audio/tests/ResamplerTests.cpp
        src/infrastructure/audio/tests/WavHeaderTests.cpp
        src/infrastructure/files/tests/WavWriterTests.cpp
        src/infrastructure/storage/tests/RecordingLibraryTests.cpp
        src/infrastructure/storage/tests/SettingsTests.cpp
        src/infrastructure/common/tests/TextTests.cpp
        src/infrastructure/common/tests/FileIoTests.cpp
        src/infrastructure/storage/Settings.cpp
        src/infrastructure/audio/Mixer.cpp
        src/infrastructure/audio/MaConverter.cpp
        src/infrastructure/audio/WavHeader.cpp
        src/infrastructure/files/WavWriter.cpp
        src/infrastructure/storage/RecordingSidecar.cpp
        src/infrastructure/storage/RecordingLibrary.cpp
        src/infrastructure/audio/Resampler.cpp
        src/core/TranscriptText.cpp
        src/infrastructure/common/Paths.cpp
        src/infrastructure/common/Text.cpp
        src/infrastructure/common/FileIo.cpp
        src/infrastructure/common/Log.cpp
        src/infrastructure/common/HResult.cpp
    )

    target_include_directories(MeetingBuddyTests PRIVATE
        ${CMAKE_CURRENT_SOURCE_DIR}/src
    )

target_compile_definitions(MeetingBuddyTests PRIVATE
    UNICODE
    _UNICODE
    WIN32_LEAN_AND_MEAN
    NOMINMAX
    WIL_SUPPRESS_EXCEPTIONS
)

target_link_libraries(MeetingBuddyTests PRIVATE
    WIL::WIL
    GTest::gtest_main
    miniaudio
    glaze::glaze
        ole32
        uuid
        shell32
        shlwapi
        advapi32
    )

    if(MSVC)
        target_compile_options(MeetingBuddyTests PRIVATE /W4 /permissive- /utf-8 /Zc:preprocessor)
    endif()

    include(GoogleTest)
    gtest_discover_tests(MeetingBuddyTests DISCOVERY_MODE PRE_TEST)
endif()
