.PHONY: clean

# Prefer a recoverable delete where `trash` exists (macOS); fall back to rm elsewhere.
RM := $(if $(shell command -v trash),trash,rm -rf)

CLEAN_PATHS := web/node_modules web/.next web/out web/tsconfig.tsbuildinfo api/target

clean:
	$(if $(wildcard $(CLEAN_PATHS)),$(RM) $(wildcard $(CLEAN_PATHS)))
