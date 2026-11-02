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

/* Returns (status, payload): status 0 = NUL-separated fields, 1 = the file
 * could not be identified. Other statuses raise Failure. */
CAMLprim value phidentify_identify_stub(value path, value flags) {
  CAMLparam2(path, flags);
  CAMLlocal2(payload, result);
  size_t path_len = caml_string_length(path);
  /* Copy the path: the GC may move the OCaml string while the runtime lock is released. */
  uint8_t *copy = malloc(path_len ? path_len : 1);
  if (!copy) caml_raise_out_of_memory();
  memcpy(copy, String_val(path), path_len);
  uint32_t f = (uint32_t)Int_val(flags);
  uint8_t *out = NULL; size_t len = 0;
  caml_release_runtime_system();
  int32_t code = phidentify_identify(copy, path_len, f, &out, &len);
  caml_acquire_runtime_system();
  free(copy);
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
