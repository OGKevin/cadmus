#! /bin/sh

TRIPLE=arm-linux-gnueabihf
export CC=${TRIPLE}-gcc
export CXX=${TRIPLE}-g++
export CFLAGS='-O2 -mcpu=cortex-a9 -mfpu=neon'
export CXXFLAGS="$CFLAGS"
export AS=${TRIPLE}-as

./autogen.sh --host=${TRIPLE} && make
