#include <onda.h>

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

typedef struct {
  const char* path;
  int element_type;
  const void* data;
  int32_t element_count;
} tensor_binding_t;

static int views_from_metadata(
  const onda_program_t* program,
  int event_index,
  const tensor_binding_t* bindings,
  size_t binding_count,
  onda_event_tensor_view_t* views,
  size_t view_capacity,
  int* view_count
) {
  const int tensor_count = onda_event_tensor_count(program, event_index);
  if (tensor_count < 0 || (size_t) tensor_count > view_capacity)
    return -1;
  for (int tensor_index = 0; tensor_index < tensor_count; ++tensor_index) {
    onda_event_tensor_info_t info;
    if (onda_event_tensor_info(program, event_index, tensor_index, &info) != 0)
      return -1;
    printf("tensor[%d] %s: type=%d shape=[", tensor_index, info.path, info.element_type);
    for (int axis = 0; axis < info.shape_rank; ++axis)
      printf("%s%d", axis == 0 ? "" : ",", info.shape[axis]);
    printf("] slice=%d fixed_elements=%d\n", info.is_slice, info.fixed_element_count);
    int found = 0;
    for (size_t i = 0; i < binding_count; ++i) {
      if (strcmp(bindings[i].path, info.path) != 0)
        continue;
      if (bindings[i].element_type != info.element_type) {
        fprintf(
          stderr,
          "host tensor '%s' has primitive type %d; event requires %d\n",
          info.path,
          bindings[i].element_type,
          info.element_type
        );
        return -1;
      }
      const int32_t count = bindings[i].element_count;
      if ((!info.is_slice && count != info.fixed_element_count)
          || (info.is_slice
              && (count < 0 || count % info.fixed_element_count != 0))) {
        fprintf(stderr, "host tensor '%s' has an incompatible shape\n", info.path);
        return -1;
      }
      views[tensor_index] = (onda_event_tensor_view_t) {
        .data = bindings[i].data,
        .element_count = count,
      };
      found = 1;
      break;
    }
    if (!found) {
      fprintf(stderr, "no host tensor bound for event leaf '%s'\n", info.path);
      return -1;
    }
  }
  *view_count = tensor_count;
  return 0;
}

static int report_diagnostic(const char* operation, onda_diag_t* diagnostic) {
  fprintf(
    stderr,
    "%s failed: %s\n",
    operation,
    diagnostic->message == NULL ? "unknown error" : diagnostic->message
  );
  onda_diag_dispose(diagnostic);
  return -1;
}

static int run_case(
  const onda_program_t* program,
  int event_index,
  const char* label,
  const onda_event_tensor_view_t* views,
  int view_count,
  int unchecked
) {
  onda_diag_t diagnostic = { 0 };
  onda_instance_t* instance =
    onda_instance_create_initialized(program, 0, 1, NULL, &diagnostic);
  if (instance == NULL)
    return report_diagnostic("instance creation", &diagnostic);
  onda_diag_dispose(&diagnostic);

  float output = 0.0F;
  if (onda_bind_output(instance, 0, &output, sizeof output) != 0
      || onda_validate_buffers(instance) != 0) {
    fprintf(stderr, "binding preparation failed\n");
    onda_instance_destroy(instance);
    return -1;
  }

  const int event_status = unchecked
    ? onda_trigger_event_views_by_index_unchecked(instance, event_index, views, NULL)
    : onda_trigger_event_views_by_index(instance, event_index, views, view_count, NULL);
  const int process_status = event_status == ONDA_EXECUTION_OK
    ? onda_process_checked(instance, 1, NULL)
    : event_status;
  printf(
    "%s: event=%d process=%d output=%.1f\n",
    label,
    event_status,
    process_status,
    output
  );
  onda_instance_destroy(instance);
  return process_status == ONDA_EXECUTION_OK && fabsf(output - 29.5F) < 1e-6F ? 0 : -1;
}

int main(void) {
  static const char source[] =
    "struct Point:\n"
    "  x: f32\n"
    "  y: f32\n"
    "outs:\n"
    "  out1\n"
    "event load(gain: f32, a: Point[], tag: i32, b: Point[]):\n"
    "  held = gain + a[0].x + a[1].y + f32(a.len()) + f32(tag)"
    " + b[0].x + b[1].y + f32(b.len())\n"
    "init:\n"
    "  held = 0.0\n"
    "sample:\n"
    "  out1 = held\n";
  const onda_compile_options_t options = {
    .sample_rate = 48000.0F,
    .block_size = 1,
  };
  onda_diag_t diagnostic = { 0 };
  onda_program_t* program = onda_compile(source, &options, &diagnostic);
  if (program == NULL)
    return report_diagnostic("compilation", &diagnostic);
  onda_diag_dispose(&diagnostic);

  const int event_index = onda_event_index(program, "load");
  if (event_index < 0) {
    fprintf(stderr, "event metadata lookup failed\n");
    onda_program_destroy(program);
    return 1;
  }

  const float gain = 0.5F;
  const float a_x[] = { 1.0F, 2.0F };
  const float a_y[] = { 3.0F, 4.0F };
  const int32_t tag = 5;
  const float b_x[] = { 6.0F, 8.0F };
  const float b_y[] = { 7.0F, 9.0F };

  const tensor_binding_t bindings[] = {
    { "gain", ONDA_PRIMITIVE_F32, &gain, 1 },
    { "a.x", ONDA_PRIMITIVE_F32, a_x, 2 },
    { "a.y", ONDA_PRIMITIVE_F32, a_y, 2 },
    { "tag", ONDA_PRIMITIVE_I32, &tag, 1 },
    { "b.x", ONDA_PRIMITIVE_F32, b_x, 2 },
    { "b.y", ONDA_PRIMITIVE_F32, b_y, 2 },
  };
  onda_event_tensor_view_t metadata_views[6];
  int metadata_view_count;
  const int metadata_result = views_from_metadata(
    program,
    event_index,
    bindings,
    sizeof bindings / sizeof bindings[0],
    metadata_views,
    sizeof metadata_views / sizeof metadata_views[0],
    &metadata_view_count
  );

  const onda_event_tensor_view_t static_views[] = {
    { &gain, 1 },
    { a_x, 2 },
    { a_y, 2 },
    { &tag, 1 },
    { b_x, 2 },
    { b_y, 2 },
  };
  const int failed = metadata_result != 0
    || run_case(
         program,
         event_index,
         "metadata + checked",
         metadata_views,
         metadata_view_count,
         0
       ) != 0
    || run_case(
         program,
         event_index,
         "static + unchecked",
         static_views,
         sizeof static_views / sizeof static_views[0],
         1
       ) != 0;
  onda_program_destroy(program);
  return failed;
}
