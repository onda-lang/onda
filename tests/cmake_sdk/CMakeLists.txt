cmake_minimum_required(VERSION 3.16)

project(onda-cmake-sdk-smoke LANGUAGES CXX)

if(MSVC)
    set(CMAKE_MSVC_RUNTIME_LIBRARY
        "MultiThreaded$<$<CONFIG:Debug>:Debug>")
endif()

set(ONDA_CMAKE_SMOKE_SOURCE "${CMAKE_CURRENT_SOURCE_DIR}/main.cpp")

function(add_onda_cmake_sdk_smoke target onda_target)
    add_library("${target}" MODULE "${ONDA_CMAKE_SMOKE_SOURCE}")
    target_compile_features("${target}" PRIVATE cxx_std_17)
    target_link_libraries("${target}" PRIVATE "${onda_target}")
    set_target_properties("${target}" PROPERTIES
        CXX_EXTENSIONS OFF
        CXX_VISIBILITY_PRESET hidden
        LIBRARY_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}"
        RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}")
    if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
        target_link_options("${target}" PRIVATE "LINKER:--no-undefined")
    endif()
endfunction()

# Keep these as sibling consumers. Imported targets are directory-scoped, so
# this catches package configs which incorrectly suppress a sibling's
# find_package call with a global include guard.
add_subdirectory(static_consumer)
add_subdirectory(shared_consumer)
