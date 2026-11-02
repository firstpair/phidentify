/* C ABI of phidentify-ffi (rust/phidentify-ffi/src/lib.rs). */
#include <stddef.h>
#include <stdint.h>
#define PHIDENTIFY_DNG_BLOCKS 1u
#define PHIDENTIFY_LIGHTROOM 2u
#define PHIDENTIFY_METADATA 4u
int32_t phidentify_identify(const uint8_t *path, size_t path_len, uint32_t flags, uint8_t **out, size_t *out_len);
int32_t phidentify_version(uint8_t **out, size_t *out_len);
void phidentify_free(uint8_t *ptr, size_t len);
