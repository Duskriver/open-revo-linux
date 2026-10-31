obj-m += openrevo_ec.o
KDIR ?= /lib/modules/$(shell uname -r)/build

.PHONY: all test
all: vault_generated.h
	$(MAKE) -C "$(KDIR)" M="$(CURDIR)" modules

vault_generated.h: ../../../config/ec_registry.json generate_header.py
	python3 generate_header.py

test: vault_generated.h
	$(CC) -std=c11 -Wall -Wextra -Werror -Wno-unused-const-variable -o test_fan_transaction test_fan_transaction.c
	./test_fan_transaction
