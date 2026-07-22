cmake_minimum_required(VERSION 3.16)
project(SoftMap C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

add_executable(softmap
    src/main.c
    src/util/util.c
    src/core/config.c
    src/core/filter.c
    src/core/tree.c
    src/core/snapshot.c
    src/scan/registry.c
    src/scan/walker.c
    src/report/report.c
    src/restore/restore.c
    src/cmd/cmd_scan.c
    src/cmd/cmd_report.c
    src/cmd/cmd_restore.c
    src/cmd/cmd_info.c
)

target_include_directories(softmap PRIVATE include)

if(WIN32)
    target_compile_definitions(softmap PRIVATE _CRT_SECURE_NO_WARNINGS)
    target_link_libraries(softmap PRIVATE advapi32)
endif()

install(TARGETS softmap RUNTIME DESTINATION bin)
install(FILES softmap.conf DESTINATION share/softmap OPTIONAL)
