# Sample AirDraw exporter plugin. Build:
#   cmake -S . -B build && cmake --build build
# then copy the library into AirDraw's plugins folder (user://plugins).
cmake_minimum_required(VERSION 3.20)
project(sample_engine_exporter CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_VISIBILITY_PRESET hidden)

add_library(sample_engine_exporter SHARED sample_engine_exporter.cpp)
target_include_directories(sample_engine_exporter PRIVATE ${CMAKE_CURRENT_LIST_DIR}/../../rust/crates/airdraw-ffi/include)
