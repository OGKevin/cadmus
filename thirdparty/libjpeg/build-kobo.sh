#! /bin/sh

TRIPLE=arm-linux-gnueabihf
export CC=${TRIPLE}-gcc
export CXX=${TRIPLE}-g++
export CFLAGS="-O2 -mcpu=cortex-a9 -mfpu=neon"

./configure --host=${TRIPLE} && make

[ -L include ] || ln -s . include
[ -L lib ] || ln -s .libs lib
