include(CMakeFindDependencyMacro)
find_dependency(Threads)

# This file is used directly from a source checkout and is also installed at
# lib/cmake/Onda/OndaConfig.cmake in an extracted SDK.
get_filename_component(_onda_source_root
    "${CMAKE_CURRENT_LIST_DIR}/.." ABSOLUTE)
get_filename_component(_onda_sdk_root
    "${CMAKE_CURRENT_LIST_DIR}/../../.." ABSOLUTE)

if(EXISTS "${_onda_source_root}/include/onda.h")
    set(_onda_root "${_onda_source_root}")
elseif(EXISTS "${_onda_sdk_root}/include/onda.h")
    set(_onda_root "${_onda_sdk_root}")
else()
    set(Onda_FOUND FALSE)
    set(Onda_NOT_FOUND_MESSAGE
        "OndaConfig.cmake could not locate include/onda.h")
    return()
endif()

if(WIN32)
    set(_onda_static_name "onda.lib")
    set(_onda_shared_name "onda.dll")
    set(_onda_import_name "onda.dll.lib")
elseif(APPLE)
    set(_onda_static_name "libonda.a")
    set(_onda_shared_name "libonda.dylib")
else()
    set(_onda_static_name "libonda.a")
    set(_onda_shared_name "libonda.so")
endif()

function(_onda_find_artifact result name)
    set(candidates
        "${_onda_root}/lib/${name}"
        "${_onda_root}/target/release/${name}")
    foreach(candidate IN LISTS candidates)
        if(EXISTS "${candidate}")
            set("${result}" "${candidate}" PARENT_SCOPE)
            return()
        endif()
    endforeach()
    set("${result}" "" PARENT_SCOPE)
endfunction()

_onda_find_artifact(_onda_static "${_onda_static_name}")
_onda_find_artifact(_onda_shared "${_onda_shared_name}")
if(WIN32)
    _onda_find_artifact(_onda_import "${_onda_import_name}")
endif()

if(NOT _onda_static)
    set(Onda_FOUND FALSE)
    string(CONCAT Onda_NOT_FOUND_MESSAGE
        "OndaConfig.cmake could not locate ${_onda_static_name} under "
        "${_onda_root}/lib or ${_onda_root}/target/release. Run "
        "'cargo build --release -p onda_api' or use a complete release SDK.")
    return()
endif()
if(NOT _onda_shared)
    set(Onda_FOUND FALSE)
    string(CONCAT Onda_NOT_FOUND_MESSAGE
        "OndaConfig.cmake could not locate ${_onda_shared_name} under "
        "${_onda_root}/lib or ${_onda_root}/target/release. Run "
        "'cargo build --release -p onda_api' or use a complete release SDK.")
    return()
endif()
if(WIN32 AND NOT _onda_import)
    set(Onda_FOUND FALSE)
    string(CONCAT Onda_NOT_FOUND_MESSAGE
        "OndaConfig.cmake could not locate ${_onda_import_name} under "
        "${_onda_root}/lib or ${_onda_root}/target/release. Use a complete "
        "release SDK.")
    return()
endif()

if(NOT TARGET Onda::Static)
    add_library(Onda::Static STATIC IMPORTED)
    set_target_properties(Onda::Static PROPERTIES
        IMPORTED_LOCATION "${_onda_static}"
        INTERFACE_INCLUDE_DIRECTORIES "${_onda_root}/include")

    set(_onda_system_libraries Threads::Threads)
    if(CMAKE_DL_LIBS)
        list(APPEND _onda_system_libraries "${CMAKE_DL_LIBS}")
    endif()
    if(WIN32)
        list(APPEND _onda_system_libraries ws2_32 userenv ntdll)
    elseif(UNIX)
        list(APPEND _onda_system_libraries m)
        if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
            list(APPEND _onda_system_libraries rt util)
        endif()
    endif()
    set_target_properties(Onda::Static PROPERTIES
        INTERFACE_LINK_LIBRARIES "${_onda_system_libraries}")

    # A static Onda SDK contains Rust and LLVM implementation symbols. Keep
    # them out of a dynamic consumer's ABI.
    if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
        set_target_properties(Onda::Static PROPERTIES
            INTERFACE_LINK_OPTIONS
                "LINKER:--exclude-libs,${_onda_static_name}")
    elseif(APPLE)
        set_target_properties(Onda::Static PROPERTIES
            INTERFACE_LINK_OPTIONS
                "LINKER:-load_hidden,${_onda_static}")
    endif()
endif()

if(NOT TARGET Onda::Shared)
    add_library(Onda::Shared SHARED IMPORTED)
    set_target_properties(Onda::Shared PROPERTIES
        IMPORTED_LOCATION "${_onda_shared}"
        INTERFACE_INCLUDE_DIRECTORIES "${_onda_root}/include")
    if(WIN32)
        set_target_properties(Onda::Shared PROPERTIES
            IMPORTED_IMPLIB "${_onda_import}")
    elseif(APPLE)
        set_target_properties(Onda::Shared PROPERTIES
            IMPORTED_SONAME "@rpath/${_onda_shared_name}")
    elseif(CMAKE_SYSTEM_NAME STREQUAL "Linux")
        set_target_properties(Onda::Shared PROPERTIES
            IMPORTED_SONAME "${_onda_shared_name}")
    endif()
endif()

unset(_onda_import)
unset(_onda_import_name)
unset(_onda_root)
unset(_onda_shared)
unset(_onda_shared_name)
unset(_onda_sdk_root)
unset(_onda_source_root)
unset(_onda_static)
unset(_onda_static_name)
unset(_onda_system_libraries)
