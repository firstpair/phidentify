/* OCaml stubs over the phidentify-ffi C ABI. Identification runs with the
 * OCaml runtime lock released, so domains and threads work in parallel. */
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <caml/mlvalues.h>
#include <caml/memory.h>
#include <caml/alloc.h>
#include <caml/fail.h>
#include <caml/threads.h>
#include "phidentify.h"

static value take_string(uint8_t *ptr, size_t len) {
  value s = caml_alloc_initialized_string(len, (const char *)ptr);
  phidentify_free(ptr, len);
  return s;
}

CAMLprim value phidentify_version_stub(value unit) {
  CAMLparam1(unit);
  uint8_t *out = NULL; size_t len = 0;
  if (phidentify_version(&out, &len) != 0) caml_failwith("phidentify version unavailable");
  CAMLreturn(take_string(out, len));
}

CAMLprim value phidentify_reads_metadata_stub(value path) {
  return Val_bool(phidentify_reads_metadata((const uint8_t *)String_val(path), caml_string_length(path)) == 1);
}

/* identify path flags regions: regions is None (no complement digest) or
 * Some of an int64 array of (offset, length) pairs. Returns (status, payload):
 * status 0 = NUL-separated fields, 1 = the file could not be identified.
 * Other statuses raise Failure. */
CAMLprim value phidentify_identify_stub(value path, value flags, value regions) {
  CAMLparam3(path, flags, regions);
  CAMLlocal2(payload, result);
  size_t path_len = caml_string_length(path);
  /* Copy the inputs: the GC may move OCaml values while the runtime lock is released. */
  uint8_t *copy = malloc(path_len ? path_len : 1);
  if (!copy) caml_raise_out_of_memory();
  memcpy(copy, String_val(path), path_len);
  uint64_t *pairs = NULL; size_t n = 0;
  if (Is_some(regions)) {
    value arr = Some_val(regions);
    size_t len = Wosize_val(arr);
    n = len / 2;
    pairs = malloc((len ? len : 1) * sizeof(uint64_t));
    if (!pairs) { free(copy); caml_raise_out_of_memory(); }
    for (size_t i = 0; i < len; i++) pairs[i] = (uint64_t)Int64_val(Field(arr, i));
  }
  uint32_t f = (uint32_t)Int_val(flags);
  uint8_t *out = NULL; size_t len = 0;
  caml_release_runtime_system();
  int32_t code = phidentify_identify(copy, path_len, f, pairs, n, &out, &len);
  caml_acquire_runtime_system();
  free(copy); free(pairs);
  if (code != 0 && code != 1) {
    char message[2048];
    snprintf(message, sizeof(message), "phidentify status %d: %.*s", (int)code,
             (int)(len < 1900 ? len : 1900), out ? (const char *)out : "");
    if (out) phidentify_free(out, len);
    caml_failwith(message);
  }
  payload = take_string(out, len);
  result = caml_alloc_tuple(2);
  Store_field(result, 0, Val_int(code));
  Store_field(result, 1, payload);
  CAMLreturn(result);
}

/* Read-only descriptor and positional reads for the tiffgraph walk, run with
 * the runtime lock released. */
#include <fcntl.h>
#include <unistd.h>
#include <errno.h>
#include <sys/stat.h>

CAMLprim value phidentify_open_ro_stub(value path) {
  CAMLparam1(path);
  CAMLlocal1(result);
  char *copy = strdup(String_val(path));
  if (!copy) caml_raise_out_of_memory();
  caml_release_runtime_system();
  int fd = open(copy, O_RDONLY | O_CLOEXEC);
  struct stat st; int ok = fd >= 0 && fstat(fd, &st) == 0;
  caml_acquire_runtime_system();
  free(copy);
  if (!ok) { if (fd >= 0) close(fd); caml_failwith("open failed"); }
  result = caml_alloc_tuple(2);
  Store_field(result, 0, Val_int(fd));
  Store_field(result, 1, caml_copy_int64((int64_t)st.st_size));
  CAMLreturn(result);
}

CAMLprim value phidentify_pread_stub(value fd, value offset, value length) {
  CAMLparam3(fd, offset, length);
  CAMLlocal1(s);
  int f = Int_val(fd); int64_t off = Int64_val(offset); long n = Long_val(length);
  if (n < 0 || n > (1L << 24)) caml_invalid_argument("pread length");
  char *buf = malloc(n ? n : 1);
  if (!buf) caml_raise_out_of_memory();
  long got = 0; int failed = 0;
  caml_release_runtime_system();
  while (got < n) {
    ssize_t r = pread(f, buf + got, n - got, off + got);
    if (r < 0 && errno == EINTR) continue;
    if (r <= 0) { failed = 1; break; }
    got += r;
  }
  caml_acquire_runtime_system();
  if (failed) { free(buf); caml_failwith("read outside the bytes"); }
  s = caml_alloc_initialized_string(n, buf);
  free(buf);
  CAMLreturn(s);
}

CAMLprim value phidentify_close_stub(value fd) {
  close(Int_val(fd));
  return Val_unit;
}
