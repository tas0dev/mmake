###################
ifndef MMAKE
$(error This Makefile requires mmake. Use 'mmake' or 'mmk' instead of 'make')
endif
###################

OUT := out
BUILD_DIR := $(OUT)/build

prepare:
	@watch Cargo.toml
	@output $(BUILD_DIR)/prepared
	mkdir -p $(BUILD_DIR)
	touch $(BUILD_DIR)/prepared

compile: prepare
	@watch src/**
	@output $(BUILD_DIR)/mmake
	cargo build --release
	cp target/release/mmake $(BUILD_DIR)/mmake

package: compile
	@watch readme.md
	@output $(OUT)/mmake.txt
	cp readme.md $(OUT)/mmake.txt

run: compile
	@always
	$(BUILD_DIR)/mmake --help

all: package
