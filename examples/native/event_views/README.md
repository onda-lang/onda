# Native event tensor views

This example sends `f32, Point[], i32, Point[]` through the zero-copy C event-view API in two ways:

- metadata-driven: enumerate `onda_event_tensor_info` records and use the checked call;
- statically known: declare views directly in schema order and use the unchecked call.

Both variants pass six contiguous SoA tensors. Their dispatch paths neither read nor write the
event workspace. The instance currently still owns its default workspace so that the packed event
API remains available without a later allocation.

On Linux:

```sh
cargo build -p onda_api
cc -std=c11 -Wall -Wextra -Werror -pedantic \
  -Iinclude examples/native/event_views/point_event_views.c \
  -Ltarget/debug -Wl,-rpath,"$PWD/target/debug" -londa \
  -lm \
  -o /tmp/onda-point-event-views
/tmp/onda-point-event-views
```
