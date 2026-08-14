# xberg-ffi CMake config-mode find module
#
# Defines the imported target:
#   xberg-ffi::xberg-ffi
#
# Usage:
#   find_package(xberg-ffi REQUIRED)
#   target_link_libraries(myapp PRIVATE xberg-ffi::xberg-ffi)

if(TARGET xberg-ffi::xberg-ffi)
return()
endif()

get_filename_component(_FFI_CMAKE_DIR "${CMAKE_CURRENT_LIST_FILE}" PATH)
get_filename_component(_FFI_PREFIX "${_FFI_CMAKE_DIR}/.." ABSOLUTE)

find_library(_FFI_LIBRARY
  NAMES xberg_ffi libxberg_ffi
  PATHS "${_FFI_PREFIX}/lib"
  NO_DEFAULT_PATH
)
if(NOT _FFI_LIBRARY)
find_library(_FFI_LIBRARY NAMES xberg_ffi libxberg_ffi)
endif()

find_path(_FFI_INCLUDE_DIR
  NAMES xberg.h
  PATHS "${_FFI_PREFIX}/include"
  NO_DEFAULT_PATH
)
if(NOT _FFI_INCLUDE_DIR)
find_path(_FFI_INCLUDE_DIR NAMES xberg.h)
endif()

include(FindPackageHandleStandardArgs)
find_package_handle_standard_args(xberg-ffi
  REQUIRED_VARS _FFI_LIBRARY _FFI_INCLUDE_DIR
)

if(xberg_ffi_FOUND)
set(_FFI_LIB_TYPE UNKNOWN)
if(_FFI_LIBRARY MATCHES "\\.(dylib|so)$" OR _FFI_LIBRARY MATCHES "\\.so\\.")
    set(_FFI_LIB_TYPE SHARED)
elseif(_FFI_LIBRARY MATCHES "\\.dll$")
    set(_FFI_LIB_TYPE SHARED)
elseif(_FFI_LIBRARY MATCHES "\\.(a|lib)$")
    set(_FFI_LIB_TYPE STATIC)
endif()

add_library(xberg-ffi::xberg-ffi ${_FFI_LIB_TYPE} IMPORTED)
    set_target_properties(xberg-ffi::xberg-ffi PROPERTIES
    IMPORTED_LOCATION "${_FFI_LIBRARY}"
    INTERFACE_INCLUDE_DIRECTORIES "${_FFI_INCLUDE_DIR}"
    )

if(WIN32 AND _FFI_LIB_TYPE STREQUAL "SHARED")
        find_file(_FFI_DLL
      NAMES xberg_ffi.dll libxberg_ffi.dll
      PATHS "${_FFI_PREFIX}/bin" "${_FFI_PREFIX}/lib"
      NO_DEFAULT_PATH
        )
    if(_FFI_DLL)
            set_target_properties(xberg-ffi::xberg-ffi PROPERTIES
        IMPORTED_LOCATION "${_FFI_DLL}"
        IMPORTED_IMPLIB "${_FFI_LIBRARY}"
            )
    endif()
    unset(_FFI_DLL CACHE)
endif()

if(APPLE)
        set_property(TARGET xberg-ffi::xberg-ffi APPEND PROPERTY
        INTERFACE_LINK_LIBRARIES "-framework CoreFoundation" "-framework Security" pthread)
elseif(UNIX)
        set_property(TARGET xberg-ffi::xberg-ffi APPEND PROPERTY
        INTERFACE_LINK_LIBRARIES pthread dl m)
elseif(WIN32)
        set_property(TARGET xberg-ffi::xberg-ffi APPEND PROPERTY
        INTERFACE_LINK_LIBRARIES ws2_32 userenv bcrypt)
endif()

unset(_FFI_LIB_TYPE)
endif()

mark_as_advanced(_FFI_LIBRARY _FFI_INCLUDE_DIR)
unset(_FFI_CMAKE_DIR)
unset(_FFI_PREFIX)
