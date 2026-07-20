# §2.2.1 — NAPI .so 构建配置
#
# 编译 libentry.so：NAPI 插件库，链接 Rust .so（libwarp.so）。
# Rust .so 通过 -L + -l 方式引用（DT_NEEDED = libwarp.so）

cmake_minimum_required(VERSION 3.20)
project(entry)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

# Workaround: redirect cmake output to a staging dir *outside* the hvigor
# intermediates tree, so hvigor's syncLibOutputs correctly copies from the
# staging dir to its expected path (no source==dest self-copy bug).
# Without this, hvigor's syncLibOutputs tries to hard-link/copy the .so to
# itself and fails with ENOENT.
set(CMAKE_LIBRARY_OUTPUT_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/../../../.cmake_staging")

# ── NAPI 源文件 ──────────────────────────────────────────────────────────────

set(SOURCES
    napi_init.cpp
    services/service_shell_bridge.cpp
    services/service_clipboard.cpp
    services/service_file_picker.cpp
    services/service_notification.cpp
    services/service_theme.cpp
    services/service_url.cpp
    services/service_display.cpp
    services/service_ime.cpp
    services/service_tsfn.cpp
    services/service_font_render.cpp
)

# ── 共享库目标 ───────────────────────────────────────────────────────────────

add_library(entry SHARED ${SOURCES})

# ── FetchContent：mbedTLS（Apache 2.0 许可证，静态链接）──────────────────

include(FetchContent)

FetchContent_Declare(
    mbedtls
    URL file://${CMAKE_CURRENT_SOURCE_DIR}/../../../../../depends/third_party_mbedtls-OpenHarmony-v3.2-Release.tar.gz
    DOWNLOAD_EXTRACT_TIMESTAMP TRUE
    SOURCE_SUBDIR third_party_mbedtls-OpenHarmony-v3.2-Release
)
set(BUILD_SHARED_LIBS OFF)
set(ENABLE_PROGRAMS OFF)
set(ENABLE_TESTING OFF)
FetchContent_MakeAvailable(mbedtls)

# OpenHarmony 移植版的顶层 CMakeLists.txt 不包含 library 子目录，
# 无法通过 add_subdirectory(library) 生成标准库目标。
# 直接收集源码编译为 OBJECT 库。
file(GLOB_RECURSE MBEDTLS_CRYPTO_SRCS
    ${mbedtls_SOURCE_DIR}/library/aes*.c ${mbedtls_SOURCE_DIR}/library/aria.c
    ${mbedtls_SOURCE_DIR}/library/asn1*.c ${mbedtls_SOURCE_DIR}/library/base64.c
    ${mbedtls_SOURCE_DIR}/library/bignum.c ${mbedtls_SOURCE_DIR}/library/camellia.c
    ${mbedtls_SOURCE_DIR}/library/ccm.c ${mbedtls_SOURCE_DIR}/library/chacha*.c
    ${mbedtls_SOURCE_DIR}/library/cipher*.c ${mbedtls_SOURCE_DIR}/library/cmac.c
    ${mbedtls_SOURCE_DIR}/library/constant_time.c ${mbedtls_SOURCE_DIR}/library/ctr_drbg.c
    ${mbedtls_SOURCE_DIR}/library/des.c ${mbedtls_SOURCE_DIR}/library/dhm.c
    ${mbedtls_SOURCE_DIR}/library/ecdh.c ${mbedtls_SOURCE_DIR}/library/ecdsa.c
    ${mbedtls_SOURCE_DIR}/library/ecjpake.c ${mbedtls_SOURCE_DIR}/library/ecp*.c
    ${mbedtls_SOURCE_DIR}/library/entropy*.c ${mbedtls_SOURCE_DIR}/library/error*.c
    ${mbedtls_SOURCE_DIR}/library/gcm*.c ${mbedtls_SOURCE_DIR}/library/gf128mul.c
    ${mbedtls_SOURCE_DIR}/library/hash_info.c ${mbedtls_SOURCE_DIR}/library/hkdf.c
    ${mbedtls_SOURCE_DIR}/library/hmac_drbg.c ${mbedtls_SOURCE_DIR}/library/lmots.c
    ${mbedtls_SOURCE_DIR}/library/lms.c ${mbedtls_SOURCE_DIR}/library/md*.c
    ${mbedtls_SOURCE_DIR}/library/memory_buffer_alloc.c
    ${mbedtls_SOURCE_DIR}/library/mps_reader.c ${mbedtls_SOURCE_DIR}/library/mps_trace.c
    ${mbedtls_SOURCE_DIR}/library/nist_kw.c ${mbedtls_SOURCE_DIR}/library/oid.c
    ${mbedtls_SOURCE_DIR}/library/padlock.c ${mbedtls_SOURCE_DIR}/library/pem.c
    ${mbedtls_SOURCE_DIR}/library/pk*.c ${mbedtls_SOURCE_DIR}/library/platform*.c
    ${mbedtls_SOURCE_DIR}/library/poly1305.c ${mbedtls_SOURCE_DIR}/library/psa_crypto*.c
    ${mbedtls_SOURCE_DIR}/library/psa_crypto_driver_wrappers_no_static.c
    ${mbedtls_SOURCE_DIR}/library/psa_its_file.c ${mbedtls_SOURCE_DIR}/library/ripemd160.c
    ${mbedtls_SOURCE_DIR}/library/rsa*.c ${mbedtls_SOURCE_DIR}/library/sha1.c
    ${mbedtls_SOURCE_DIR}/library/sha256.c ${mbedtls_SOURCE_DIR}/library/sha512.c
    ${mbedtls_SOURCE_DIR}/library/sha3.c ${mbedtls_SOURCE_DIR}/library/threading.c
    ${mbedtls_SOURCE_DIR}/library/timing.c ${mbedtls_SOURCE_DIR}/library/version*.c
)
add_library(mbedcrypto OBJECT ${MBEDTLS_CRYPTO_SRCS})
target_include_directories(mbedcrypto PUBLIC
    ${mbedtls_SOURCE_DIR}/include
    ${mbedtls_SOURCE_DIR}/library
)

file(GLOB_RECURSE MBEDTLS_X509_SRCS
    ${mbedtls_SOURCE_DIR}/library/x509*.c ${mbedtls_SOURCE_DIR}/library/certs.c
    ${mbedtls_SOURCE_DIR}/library/pkcs11.c
)
add_library(mbedx509 OBJECT ${MBEDTLS_X509_SRCS})
target_include_directories(mbedx509 PUBLIC ${mbedtls_SOURCE_DIR}/include)

file(GLOB_RECURSE MBEDTLS_TLS_SRCS
    ${mbedtls_SOURCE_DIR}/library/ssl_*.c ${mbedtls_SOURCE_DIR}/library/ssl_tls.c
    ${mbedtls_SOURCE_DIR}/library/ssl_tls13_*.c ${mbedtls_SOURCE_DIR}/library/debug.c
    ${mbedtls_SOURCE_DIR}/library/net_sockets.c
)
add_library(mbedtls OBJECT ${MBEDTLS_TLS_SRCS})
target_include_directories(mbedtls PUBLIC ${mbedtls_SOURCE_DIR}/include)

# ── FetchContent：libssh2（BSD 许可证，静态链接，使用 mbedTLS 后端）───────

FetchContent_Declare(
    libssh2
    URL file://${CMAKE_CURRENT_SOURCE_DIR}/../../../../../depends/libssh2-1.11.0.tar.gz
    DOWNLOAD_EXTRACT_TIMESTAMP TRUE
    SOURCE_SUBDIR libssh2-1.11.0
)
set(BUILD_SHARED_LIBS OFF)
set(BUILD_EXAMPLES OFF)
set(BUILD_TESTS OFF)
set(CRYPTO_BACKEND "mbedTLS")
FetchContent_MakeAvailable(libssh2)

# libssh2 的 CMake 库目标名是 libssh2_static（带 OUTPUT_NAME 自定义），
# 直接用 OBJECT 库方式编译其源文件，手动传递 mbedTLS 后端定义。
file(GLOB LIBSSH2_SRCS ${libssh2_SOURCE_DIR}/src/*.c)
add_library(libssh2_static OBJECT ${LIBSSH2_SRCS})
target_include_directories(libssh2_static PUBLIC
    ${libssh2_SOURCE_DIR}/include
    ${mbedtls_SOURCE_DIR}/include
)
target_compile_definitions(libssh2_static PRIVATE
    LIBSSH2_MBEDTLS
    LIBSSH2_NO_DEPRECATED
)
target_compile_options(libssh2_static PRIVATE -include sys/uio.h)

# ── Native 子进程库（libohos_shell.so）──────────────────────────────────────

add_library(ohos_shell SHARED
    bridge/shell_bridge_process.cpp
)

# ── 依赖：鸿蒙 NDK ──────────────────────────────────────────────────────────

target_link_libraries(entry
    PUBLIC
    ace_napi.z
    ace_ndk.z
    libhilog_ndk.z.so
    libchild_process.so
    libpasteboard.so
    libudmf.so
    libnative_drawing.so
    -Wl,--whole-archive c++ c++abi -Wl,--no-whole-archive
)

target_link_libraries(ohos_shell
    PUBLIC
    libchild_process.so
    libhilog_ndk.z.so
    mbedtls
    mbedcrypto
    mbedx509
    libssh2_static
    m
)

target_include_directories(ohos_shell PRIVATE
    ${mbedtls_SOURCE_DIR}/include
    ${libssh2_SOURCE_DIR}/include
)

# ── Rust .so：运行时 dlopen 加载 ──────────────────────────────────────
#
# libwarp.so 不通过编译期 -lwarp 链接到 libentry.so，避免设备上某些
# OHOS NDK 符号不可用时整个加载链断裂。改用运行时 dlopen(libwarp.so)
# 在 napi_init.cpp 的 Init() 中按需加载。
# XComponent（libraryname: warp）独立加载 libwarp.so，不受此影响。

# ── 头文件搜索路径 ───────────────────────────────────────────────────────────

target_include_directories(entry PRIVATE ${CMAKE_CURRENT_SOURCE_DIR})
