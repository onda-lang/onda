cmake_minimum_required(VERSION 3.16)

project(onda-cmake-sdk-smoke LANGUAGES CXX)

if(MSVC)
    set(CMAKE_MSVC_RUNTIME_LIBRARY
        "MultiThreaded$<$<CONFIG:Debug>:Debug>")
endif()

find_package(Onda CONFIG REQUIRED)

add_library(onda_cmake_static_smoke MODULE main.cpp)
target_compile_features(onda_cmake_static_smoke PRIVATE cxx_std_17)
target_link_libraries(onda_cmake_static_smoke PRIVATE Onda::Static)

add_library(onda_cmake_shared_smoke MODULE main.cpp)
target_compile_features(onda_cmake_shared_smoke PRIVATE cxx_std_17)
target_link_libraries(onda_cmake_shared_smoke PRIVATE Onda::Shared)

foreach(target
        onda_cmake_static_smoke
        onda_cmake_shared_smoke)
    set_target_properties("${target}" PROPERTIES
        CXX_EXTENSIONS OFF
        CXX_VISIBILITY_PRESET hidden)
    if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
        target_link_options("${target}" PRIVATE "LINKER:--no-undefined")
    endif()
endforeach()
