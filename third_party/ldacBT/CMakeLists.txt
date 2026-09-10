cmake_minimum_required(VERSION 3.5)
project(libldac LANGUAGES C VERSION 2.0.2.3)

set(CMAKE_C_STANDARD 99)

include_directories(libldac/inc)
include_directories(libldac/abr/inc)

if(MSVC)
    include_directories(${CMAKE_CURRENT_SOURCE_DIR}/compat)
    add_definitions(-D_CRT_SECURE_NO_WARNINGS -FImsvc_compat.h)
endif()

# libldacBT_enc - uses unity build
add_library(ldacBT_enc STATIC
    libldac/src/ldaclib.c
    libldac/src/ldacBT.c
)

# libldacBT_abr
add_library(ldacBT_abr STATIC
    libldac/abr/src/ldacBT_abr.c
)

install(TARGETS ldacBT_enc ldacBT_abr
    LIBRARY DESTINATION ${CMAKE_INSTALL_PREFIX}/lib
    PUBLIC_HEADER DESTINATION ${CMAKE_INSTALL_PREFIX}/include/ldac
)
