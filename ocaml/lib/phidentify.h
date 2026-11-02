/* C ABI of phidentify-ffi (rust/phidentify-ffi/src/lib.rs). */
#include <stddef.h>
#include <stdint.h>
#define PHIDENTIFY_DNG_BLOCKS 1u
#define PHIDENTIFY_LIGHTROOM 2u
#define PHIDENTIFY_METADATA 4u
#define PHIDENTIFY_COMPACT_RECORD 8u
#define PHIDENTIFY_IMAGE_DATA 16u
/* Fields on status 0, NUL-separated: sha256, raw blocks, Lightroom, metadata digest, preserved,
   metadata record, image data; each "ok:<value>", "err:<reason>" or empty when not attempted.
   regions: n_regions (offset, length) pairs, or NULL for no complement digest. */
int32_t phidentify_identify(const uint8_t *path, size_t path_len, uint32_t flags,
                            const uint64_t *regions, size_t n_regions, uint8_t **out, size_t *out_len);
int32_t phidentify_version(uint8_t **out, size_t *out_len);
void phidentify_free(uint8_t *ptr, size_t len);
