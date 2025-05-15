#!/bin/bash -e
# Flush stdout immediately to see build progress in real-time
export PYTHONUNBUFFERED=1
export PYTHONFAULTHANDLER=1
stdbuf -oL -eL true  # Force line buffering for all commands in this script

#  KBUILD_VERBOSE=1
cd /home/joelaf/repo/linux-acourbot-gsp
make M=drivers/gpu/nova-core -j4 2>&1 | grep -v "objtool" | grep -v die__|grep -v tag__ | grep -v 'clang diag:' | tee /tmp/make.log

sudo rmmod nova-core || true
echo 1 | sudo tee /sys/bus/pci/devices/0000:21:00.0/reset 
sudo insmod drivers/gpu/nova-core/nova_core.ko
sudo dmesg 2>&1 | tee /tmp/dmesg.log
