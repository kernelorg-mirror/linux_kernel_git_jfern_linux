# SPDX-License-Identifier: GPL-2.0+
TEST_GEN_PROGS := cs_dlserver_test
TEST_GEN_PROGS += dlserver_test

cs_dlserver_test: cs_dlserver_test.c common.c

dlserver_test: dlserver_test.c

CFLAGS += $(KHDR_INCLUDES)
CFLAGS += -Wall

include ../lib.mk
