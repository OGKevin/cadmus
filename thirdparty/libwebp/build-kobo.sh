#!/bin/sh

TRIPLE=arm-linux-gnueabihf
export CC=${TRIPLE}-gcc
export CXX=${TRIPLE}-g++
export CFLAGS="-O2 -mcpu=cortex-a9 -mfpu=neon"
export CXXFLAGS="$CFLAGS"

if [ ! -f configure ]; then
  NOCONFIGURE=1 sh autogen.sh
fi

./configure \
  --host=${TRIPLE} \
  --enable-shared \
  --disable-static \
  --disable-libwebpmux \
  --enable-libwebpdecoder \
  --enable-libwebpdemux \
  --disable-webp-tools &&
  make
