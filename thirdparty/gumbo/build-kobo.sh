#! /bin/sh

export TRIPLE=arm-linux-gnueabihf
export CC=${TRIPLE}-gcc
export CXX=${TRIPLE}-g++

[ -x configure ] || ./autogen.sh
./configure --host="$TRIPLE" && make
