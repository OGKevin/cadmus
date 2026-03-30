#! /bin/sh

echo 0 > /sys/power/state-extended
case "$PRODUCT" in
	alyssum|dahlia)
		echo a > /sys/devices/virtual/input/input1/neocmd
		;;
esac
