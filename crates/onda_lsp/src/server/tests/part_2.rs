    #[test]
    fn navigation_resolves_namespace_local_const_use_through_lsp_server_path() {
        let dir = mk_temp_dir("navigation_namespace_local_const_use_lsp");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP:
  const Bias = 0.5

  def shape(x):
    return x + Bias
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "return x + Bias");
        let hover =
            hover_markdown_for(&mut server, &main, source, "return x + Bias").unwrap_or_default();

        assert_eq!(definition["range"]["start"]["line"], json!(2));
        assert!(
            hover.contains("const Bias"),
            "namespace-local const hover should resolve at use site: {hover:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_resolves_namespace_local_const_in_generic_def_through_lsp_server_path() {
        let dir = mk_temp_dir("navigation_namespace_generic_const_use_lsp");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  const TEST = 10
  
  def sampleDuration<T>():
    return T(1.0) / T(SR)

  def blockDuration<T>():
    return T(BS) / T(SR) * TEST
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "* TEST");
        let hover = hover_markdown_for(&mut server, &main, source, "* TEST").unwrap_or_default();

        assert_eq!(
            definition["range"]["start"]["line"],
            json!(2),
            "namespace-local const in generic def should resolve through server path: {definition:?}"
        );
        assert!(
            hover.contains("const TEST"),
            "namespace-local const hover should resolve in generic def: {hover:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_resolves_current_namespace_const_with_stale_parse_cache() {
        let dir = mk_temp_dir("navigation_namespace_const_stale_parse");
        let main = super::normalize_path(&dir.join("main.onda"));
        let old_source = r#"
namespace sc:
  def blockDuration<T>():
    return T(BS) / T(SR)
"#;
        let current_source = r#"
namespace sc:
  const TEST = 10
  
  def sampleDuration<T>():
    return T(1.0) / T(SR)

  def blockDuration<T>():
    return T(BS) / T(SR) * TEST
"#;
        write_file(&main, old_source);

        let mut server = LspServer::default();
        let old_parsed =
            onda_frontend::parse_program_file_with_overlays(&main, &server.session.overlay_map())
                .expect("old source should parse");
        server.cache_parsed_program_for_path(
            &main,
            super::source_fingerprint(old_source),
            Some(old_parsed),
        );
        write_file(&main, current_source);

        let definition = definition_for(&mut server, &main, current_source, "* TEST");
        let hover =
            hover_markdown_for(&mut server, &main, current_source, "* TEST").unwrap_or_default();

        assert_eq!(
            definition["range"]["start"]["line"],
            json!(2),
            "current namespace const should resolve despite stale parse: {definition:?}"
        );
        assert!(
            hover.contains("const TEST"),
            "current namespace const hover should resolve despite stale parse: {hover:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_lists_struct_self_members_typed_param_members_and_constructor_args() {
        let dir = mk_temp_dir("completion_struct_members_args");
        let main = dir.join("main.onda");
        let self_source = r#"
struct Box:
  value: f32 = 0.0

  def set(self, x):
    self.value = x

  def get(self):
    return self.value

  def complete_self(self):
    self.
"#;
        let self_arg_source = r#"
struct Box:
  value: f32 = 0.0

  def set(self, x):
    self.value = x

  def complete_self_args(self):
    self.set(
"#;
        let typed_param_source = r#"
struct Box:
  value: f32 = 0.0

  def get(self):
    return self.value

def read(item: Box):
  return item.
"#;
        let ctor_source = r#"
struct Box:
  value: f32 = 0.0

init:
  b = Box(
"#;
        write_file(&main, self_source);

        let mut server = LspServer::default();
        let self_labels = completion_labels_for(&mut server, &main, self_source, "self.");
        assert!(
            self_labels.contains(&"value".to_owned()),
            "labels: {self_labels:?}"
        );
        assert!(
            self_labels.contains(&"set".to_owned()),
            "labels: {self_labels:?}"
        );

        let arg_labels = completion_labels_for(&mut server, &main, self_arg_source, "self.set(");
        assert!(
            arg_labels.contains(&"x".to_owned()),
            "labels: {arg_labels:?}"
        );

        let param_labels = completion_labels_for(&mut server, &main, typed_param_source, "item.");
        assert!(
            param_labels.contains(&"value".to_owned()),
            "labels: {param_labels:?}"
        );
        assert!(
            param_labels.contains(&"get".to_owned()),
            "labels: {param_labels:?}"
        );

        let ctor_labels = completion_labels_for(&mut server, &main, ctor_source, "Box(");
        assert!(
            ctor_labels.contains(&"value".to_owned()),
            "labels: {ctor_labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_orders_namespace_members_by_declaration_kind() {
        let dir = mk_temp_dir("completion_namespace_member_order");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP:
  namespace Zoo:
    const X = 1

  namespace Alpha:
    const X = 1

  struct ZStruct:
    value: f32

  struct AStruct:
    value: f32

  proc ZProc:
    outs:
      out1
    sample:
      out1 = 0.0

  proc AProc:
    outs:
      out1
    sample:
      out1 = 0.0

  def zdef():
    return 0.0

  def adef():
    return 0.0

  const ZConst = 1
  const AConst = 1

outs:
  out1

sample:
  out1 = DSP::
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "DSP::");
        let expected = vec![
            "Alpha", "Zoo", "AProc", "ZProc", "AStruct", "ZStruct", "adef", "zdef", "AConst",
            "ZConst",
        ];
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .filter(|label| expected.contains(label))
            .collect::<Vec<_>>();

        assert_eq!(labels, expected, "items: {items:?}");
        for (label, sort_text) in [
            ("Alpha", "00_00_Alpha"),
            ("AProc", "00_01_AProc"),
            ("AStruct", "00_02_AStruct"),
            ("adef", "00_10_adef"),
            ("AConst", "00_11_AConst"),
        ] {
            let item = items
                .iter()
                .find(|item| item["label"] == json!(label))
                .unwrap_or_else(|| panic!("missing {label} in {items:?}"));
            assert_eq!(item["sortText"], json!(sort_text), "item: {item:?}");
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_prefers_matching_case_then_declaration_kind() {
        let dir = mk_temp_dir("completion_case_aware_order");
        let main = dir.join("main.onda");
        let source = r#"
namespace Cases:
  namespace SNamespace:
    const X = 1

  namespace sNamespace:
    const X = 1

  proc SProc:
    outs:
      out1
    sample:
      out1 = 0.0

  proc sProc:
    outs:
      out1
    sample:
      out1 = 0.0

  struct SStruct:
    value: f32

  struct sStruct:
    value: f32

  def SDef():
    return 0.0

  def sDef():
    return 0.0

  const SConst = 1
  const sConst = 1

outs:
  out1

sample:
  out1 = Cases::S
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "Cases::S");
        let expected = vec![
            "SNamespace",
            "SProc",
            "SStruct",
            "SDef",
            "SConst",
            "sNamespace",
            "sProc",
            "sStruct",
            "sDef",
            "sConst",
        ];
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .filter(|label| expected.contains(label))
            .collect::<Vec<_>>();

        assert_eq!(labels, expected, "items: {items:?}");
        for (label, sort_text) in [
            ("SNamespace", "0_00_00_SNamespace"),
            ("SProc", "0_00_01_SProc"),
            ("sNamespace", "1_00_00_sNamespace"),
            ("sProc", "1_00_01_sProc"),
        ] {
            let item = items
                .iter()
                .find(|item| item["label"] == json!(label))
                .unwrap_or_else(|| panic!("missing {label} in {items:?}"));
            assert_eq!(item["sortText"], json!(sort_text), "item: {item:?}");
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_requeries_after_empty_general_prefix_before_case_ranking() {
        let dir = mk_temp_dir("completion_case_aware_requery");
        let main = dir.join("main.onda");
        let empty_prefix_source = r#"import std/osc
use std::osc

init:
  osc = Sine()
  tosc = 

sample:
  out1 = osc()
"#;
        write_file(&main, empty_prefix_source);

        let mut server = LspServer::default();
        let normalized = server.session.open_document(
            &main,
            onda_daemon::DocumentVersion(1),
            empty_prefix_source,
        );
        let initial = server
            .completions_for_uri(
                &path_to_file_uri(&normalized),
                position_after(empty_prefix_source, "tosc = "),
                None,
            )
            .expect("initial completion should succeed");
        assert_eq!(
            initial["isIncomplete"],
            json!(true),
            "the client must requery after the first identifier character: {initial:?}"
        );

        let typed_source = empty_prefix_source.replacen("tosc = ", "tosc = S", 1);
        let labels = completion_labels_for(&mut server, &main, &typed_source, "tosc = S");
        assert_eq!(
            labels.first().map(String::as_str),
            Some("Saw"),
            "{labels:?}"
        );
        let std_position = labels
            .iter()
            .position(|label| label == "std")
            .expect("lowercase std fallback");
        assert!(
            std_position > 0,
            "uppercase matches must precede std: {labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_orders_scope_symbols_before_declarations_and_events_after_defs() {
        let dir = mk_temp_dir("completion_scope_symbol_order");
        let main = dir.join("main.onda");
        let source = r#"
namespace gNs:
  const X = 1

struct hStruct:
  value: f32

proc iProc:
  outs:
    out1
  sample:
    out1 = 0.0

def j_def():
  return 0.0

const zConst = 1

proc Voice:
  ins:
    c_in

  buffers:
    b_buf: f32

  params:
    e_param = 0.0

  outs:
    d_out

  events:
    k_event():
      e_param = 1.0

  def f_proc_def():
    return e_param

  sample:
    a_local = 0.0
    d_out =
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "d_out =");
        let expected = vec![
            "a_local",
            "b_buf",
            "c_in",
            "d_out",
            "e_param",
            "f_proc_def",
            "gNs",
            "hStruct",
            "iProc",
            "j_def",
            "k_event",
            "zConst",
        ];
        let labels = items
            .iter()
            .filter_map(|item| item["label"].as_str())
            .filter(|label| expected.contains(label))
            .collect::<Vec<_>>();

        assert_eq!(labels, expected, "items: {items:?}");
        for (label, sort_text) in [
            ("a_local", "00_a_local"),
            ("b_buf", "01_b_buf"),
            ("f_proc_def", "02_f_proc_def"),
            ("gNs", "10_gNs"),
            ("j_def", "13_j_def"),
            ("k_event", "14_k_event"),
            ("zConst", "15_zConst"),
        ] {
            let item = items
                .iter()
                .find(|item| item["label"] == json!(label))
                .unwrap_or_else(|| panic!("missing {label} in {items:?}"));
            assert_eq!(item["sortText"], json!(sort_text), "item: {item:?}");
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_use_namespace_aliases() {
        let dir = mk_temp_dir("completion_use_namespace_alias");
        let main = dir.join("main.onda");
        let source = r#"
import std/fft
use std::fft<8> as fft8

outs:
  out1

sample:
  out1 = fft8::
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let labels = completion_labels_for(&mut server, &main, source, "fft8::");

        assert!(labels.contains(&"FFT".to_owned()), "labels: {labels:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_shows_generics_for_generic_symbols() {
        let dir = mk_temp_dir("completion_generics");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP<N = 4>:
  struct Shape<T>:
    x: T

  proc Voice<T>:
    outs:
      out1
    sample:
      out1 = 0.0

  namespace Inner<M = 2>:
    const X = 1

  def run(x):
    return V
"#;
        write_file(&main, source);

        let mut server = LspServer {
            completion_snippets: true,
            ..LspServer::default()
        };
        let items = completion_items_for(&mut server, &main, source, "return V");
        let voice = items
            .iter()
            .find(|item| item["label"] == json!("Voice"))
            .expect("Voice completion item");

        assert_eq!(voice["labelDetails"]["detail"], json!("<T>()"));
        assert_eq!(voice["insertText"], json!("Voice<${1:T}>($2)"));
        assert_eq!(voice["insertTextFormat"], json!(2));
        assert!(
            voice["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("Voice<T>"),
            "item: {voice:?}"
        );

        let items = completion_items_for(&mut server, &main, source, "return ");
        let shape = items
            .iter()
            .find(|item| item["label"] == json!("Shape"))
            .expect("Shape completion item");
        let inner = items
            .iter()
            .find(|item| item["label"] == json!("Inner"))
            .expect("Inner completion item");

        assert_eq!(shape["labelDetails"]["detail"], json!("<T>"));
        assert_eq!(shape["insertText"], json!("Shape<${1:T}>"));
        assert_eq!(shape["insertTextFormat"], json!(2));
        assert!(
            shape["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("Shape<T>"),
            "item: {shape:?}"
        );
        assert_eq!(inner["labelDetails"]["detail"], json!("<M>"));
        assert_eq!(inner["insertText"], json!("Inner<${1:M}>"));
        assert_eq!(inner["insertTextFormat"], json!(2));
        assert!(
            inner["detail"]
                .as_str()
                .unwrap_or_default()
                .contains("Inner<M>"),
            "item: {inner:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_inserts_std_complex_generic_snippet() {
        let dir = mk_temp_dir("completion_std_complex_generic");
        let main = dir.join("main.onda");
        let source = r#"
import std/complex

outs:
  out1

init:
  z: std::complex::C

sample:
  out1 = 0.0
"#;
        write_file(&main, source);

        let mut server = LspServer {
            completion_snippets: true,
            ..LspServer::default()
        };
        let items = completion_items_for(&mut server, &main, source, "std::complex::C");
        let complex = items
            .iter()
            .find(|item| item["label"] == json!("Complex"))
            .expect("Complex completion item");

        assert_eq!(complex["labelDetails"]["detail"], json!("<T>"));
        assert_eq!(complex["insertText"], json!("Complex<${1:T}>"));
        assert_eq!(complex["insertTextFormat"], json!(2));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_keeps_generics_without_snippets_and_no_fake_constructor_namespace() {
        let dir = mk_temp_dir("completion_generic_plain_text");
        let main = dir.join("main.onda");
        let source = r#"
import std/convolution

namespace Test<FFTSize = 64, MaxImpulseLen = 1024>:
  struct Box<T>:
    value: T

  def id<T>(x: T):
    return x

  def make<T>(x: T):
    local: B
    return i

  proc Use<T>:
    init:
      conv = std::convolution<FFTSize, MaxImpulseLen>::BlockConv
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let items = completion_items_for(&mut server, &main, source, "BlockConv");
        let block_convolver_items = items
            .iter()
            .filter(|item| item["label"] == json!("BlockConvolver"))
            .collect::<Vec<_>>();
        assert_eq!(
            block_convolver_items.len(),
            1,
            "BlockConvolver should have one completion item: {items:?}"
        );
        let block_convolver = block_convolver_items[0];
        assert_eq!(block_convolver["kind"], json!(4));
        assert_eq!(block_convolver["labelDetails"]["detail"], json!("<T>()"));
        assert_eq!(block_convolver["insertText"], json!("BlockConvolver<T>("));
        assert!(
            !items
                .iter()
                .any(|item| item["label"] == json!("BlockConvolver") && item["kind"] == json!(9)),
            "constructor should not also appear as a namespace: {items:?}"
        );

        let items = completion_items_for(&mut server, &main, source, "local: B");
        let box_item = items
            .iter()
            .find(|item| item["label"] == json!("Box"))
            .expect("Box completion item");
        assert_eq!(box_item["insertText"], json!("Box<T>"));

        let items = completion_items_for(&mut server, &main, source, "return i");
        let id_item = items
            .iter()
            .find(|item| item["label"] == json!("id"))
            .expect("id completion item");
        assert_eq!(id_item["insertText"], json!("id<T>("));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hover_describes_user_defs() {
        let dir = mk_temp_dir("hover_user_defs");
        let main = dir.join("main.onda");
        let source = r#"
def scale(x):
  return x

outs:
  out1

sample:
  out1 = scale(0.5)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let hover = hover_markdown_for(&mut server, &main, source, "scale")
            .expect("hover should resolve user def");

        assert!(hover.contains("def scale(x)"), "hover: {hover}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hover_describes_builtin_calls() {
        let dir = mk_temp_dir("hover_builtin_calls");
        let main = dir.join("main.onda");
        let source = r#"
init:
  buf: f32[4]
  n = buf.len()

buffers:
  clip: f32

sample:
  x = buf[0]
  y = fabs(0.0 - 1.0)
  sr = HOST_SR
  clip_bound = clip.bound()
  out1 = x + y + sr
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let len = hover_markdown_for(&mut server, &main, source, "len")
            .expect("hover should resolve len builtin");
        let bound = hover_markdown_for(&mut server, &main, source, "bound")
            .expect("hover should resolve bound builtin");
        let fabs = hover_markdown_for(&mut server, &main, source, "fabs")
            .expect("hover should resolve fabs builtin alias");
        let host_sr = hover_markdown_for(&mut server, &main, source, "HOST_SR")
            .expect("hover should resolve HOST_SR builtin const");

        assert!(len.contains("built-in call .len(...)"), "hover: {len}");
        assert!(
            bound.contains("built-in call .bound(...)"),
            "hover: {bound}"
        );
        assert!(fabs.contains("built-in call fabs(...)"), "hover: {fabs}");
        assert!(
            host_sr.contains("builtin const HOST_SR: f32"),
            "hover: {host_sr}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn callable_display_includes_argument_signatures() {
        let dir = mk_temp_dir("callable_display_argument_signatures");
        let main = dir.join("main.onda");
        let source = r#"
def scale<T>(x: T, amount: f32 = 1.0):
  return x

proc Voice<T>:
  ins<T>:
    input

  params:
    private cutoff: f32 = 1000.0
    gain: f32 = 1.0

  buffers:
    table: buffer<f32>

  event set(v: f32 = 0.5):
    gain = v

  sample:
    out1 = input * gain

init:
  voice = Voice<f32>(cutoff = 800.0, gain = 0.25, table = table)

sample:
  voice.init(gain = 0.5)
  voice.set(v = 0.75)
  out1 = voice(0.1, gain = 0.5) + scale<f32>(0.25)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let def_hover = hover_markdown_for(&mut server, &main, source, "scale")
            .expect("hover should resolve generic def");
        let constructor_hover = hover_markdown_for(&mut server, &main, source, "Voice")
            .expect("hover should resolve constructor");
        let call_hover = hover_markdown_for(&mut server, &main, source, "voice")
            .expect("hover should resolve proc call");
        let init_hover = hover_markdown_for(&mut server, &main, source, "init")
            .expect("hover should resolve proc init");
        let event_hover = hover_markdown_for(&mut server, &main, source, "set")
            .expect("hover should resolve event");

        assert!(
            def_hover.contains("def scale<T>(x: T, amount: f32 = 1.0)"),
            "hover: {def_hover}"
        );
        assert!(
            constructor_hover.contains(
                "proc Voice<T>(private cutoff: f32 = 1000.0, gain: f32 = 1.0, table: buffer<f32>)"
            ),
            "hover: {constructor_hover}"
        );
        assert!(
            call_hover.contains("proc call voice(input: T, gain: f32 = 1.0)"),
            "hover: {call_hover}"
        );
        assert!(
            init_hover.contains("event init(private cutoff: f32 = 1000.0, gain: f32 = 1.0)"),
            "hover: {init_hover}"
        );
        assert!(
            event_hover.contains("event set(v: f32 = 0.5)"),
            "hover: {event_hover}"
        );

        let def_items = completion_items_for(&mut server, &main, source, "scale");
        let scale_item = def_items
            .iter()
            .find(|item| item["label"] == json!("scale"))
            .expect("scale completion item");
        assert_eq!(
            scale_item["labelDetails"]["detail"],
            json!("<T>(x: T, amount: f32 = 1.0)")
        );
        assert_eq!(
            scale_item["detail"],
            json!("def scale<T>(x: T, amount: f32 = 1.0)")
        );

        let member_items = completion_items_for(&mut server, &main, source, "voice.");
        let init_item = member_items
            .iter()
            .find(|item| item["label"] == json!("init"))
            .expect("init completion item");
        let set_item = member_items
            .iter()
            .find(|item| item["label"] == json!("set"))
            .expect("set completion item");
        assert_eq!(
            init_item["detail"],
            json!("event init(private cutoff: f32 = 1000.0, gain: f32 = 1.0)")
        );
        assert_eq!(set_item["detail"], json!("event set(v: f32 = 0.5)"));
        assert_eq!(
            init_item["labelDetails"]["detail"],
            json!("(private cutoff: f32 = 1000.0, gain: f32 = 1.0)")
        );
        assert_eq!(set_item["labelDetails"]["detail"], json!("(v: f32 = 0.5)"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn completion_resolves_relative_namespaced_proc_instance_call_args() {
        let dir = mk_temp_dir("completion_relative_namespaced_proc_instance_args");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace Convolution2<FFTSize = 2048, MaxKernel = 8192>:
    namespace Mono:
      proc ar<T>:
        ins<T>:
          in1
          trigger

        params:
          gain: f32 = 1.0

        buffers:
          kernel: buffer<T>

        sample:
          out1 = in1 * gain

  namespace Convolution3<FFTSize = 2048, MaxKernel = 8192>:
    namespace Mono:
      proc ar<T>:
        ins<T>:
          in1
          trigger

        buffers:
          kernel: buffer<T>

        init:
          conv = Convolution2<FFTSize, MaxKernel>::Mono::ar<T>(kernel = kernel)

        sample:
          conv.init(gain = 0.5)
          out1 = conv(in1, trigger)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let call_labels = completion_labels_for(&mut server, &main, source, "conv(");
        assert!(
            call_labels.contains(&"in1".to_owned()),
            "labels: {call_labels:?}"
        );
        assert!(
            call_labels.contains(&"trigger".to_owned()),
            "labels: {call_labels:?}"
        );
        assert!(
            call_labels.contains(&"gain".to_owned()),
            "labels: {call_labels:?}"
        );

        let init_labels = completion_labels_for(&mut server, &main, source, "conv.init(");
        assert!(
            init_labels.contains(&"gain".to_owned()),
            "labels: {init_labels:?}"
        );

        let ctor_labels = completion_labels_for(&mut server, &main, source, "Mono::ar<T>(kernel");
        assert!(
            ctor_labels.contains(&"kernel".to_owned()),
            "labels: {ctor_labels:?}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_current_namespace_symbols() {
        let dir = mk_temp_dir("definition_namespace");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP:
  const Bias = 0.5

  def shape(x):
    return x + Bias
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "Bias");

        assert_ne!(definition, json!(null), "definition should resolve");
        assert_eq!(definition["range"]["start"]["line"], json!(2));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_top_level_consts() {
        let dir = mk_temp_dir("definition_top_level_const");
        let main = dir.join("main.onda");
        let source = r#"
const Scale = 0.5

outs:
  out1

sample:
  out1 = Scale
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "Scale");

        assert_ne!(definition, json!(null), "top-level const should resolve");
        assert_eq!(definition["range"]["start"]["line"], json!(1));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_treats_first_qualified_segment_as_namespace() {
        let dir = mk_temp_dir("navigation_qualified_namespace_segment");
        let main = dir.join("main.onda");
        let source = concat!(
            "namespace mode:\n",
            "  const LOW = 0\n",
            "\n",
            "proc Filter:\n",
            "  params:\n",
            "    mode: i32 = mode::LOW\n",
            "\n",
            "  outs:\n",
            "    out1\n",
            "    out2\n",
            "\n",
            "  sample:\n",
            "    out1 = mode::LOW\n",
            "    out2 = mode\n",
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let qualified_definition = definition_for(&mut server, &main, source, "out1 = mode");
        let bare_definition = definition_for(&mut server, &main, source, "out2 = mode");
        let qualified_hover =
            hover_markdown_for(&mut server, &main, source, "out1 = mode").unwrap_or_default();
        let bare_hover =
            hover_markdown_for(&mut server, &main, source, "out2 = mode").unwrap_or_default();

        assert_ne!(
            qualified_definition,
            json!(null),
            "qualified namespace segment should resolve"
        );
        assert_eq!(qualified_definition["range"]["start"]["line"], json!(0));
        assert_ne!(
            bare_definition,
            json!(null),
            "bare param reference should resolve"
        );
        assert_eq!(bare_definition["range"]["start"]["line"], json!(5));
        assert!(
            qualified_hover.contains("namespace mode"),
            "hover: {qualified_hover}"
        );
        assert!(
            bare_hover.contains("proc param mode"),
            "hover: {bare_hover}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn navigation_qualified_namespace_segment_ignores_value_symbols() {
        let dir = mk_temp_dir("navigation_qualified_namespace_segment_ignores_values");
        let main = dir.join("main.onda");
        let source = concat!(
            "namespace mode:\n",
            "  const LOW = 0\n",
            "\n",
            "namespace outer:\n",
            "  const mode = 1\n",
            "\n",
            "  proc Filter:\n",
            "    params:\n",
            "      mode: i32 = 0\n",
            "\n",
            "    outs:\n",
            "      out1\n",
            "      out2\n",
            "\n",
            "    sample:\n",
            "      out1 = mode::LOW\n",
            "      out2 = mode\n",
        );
        write_file(&main, source);

        let mut server = LspServer::default();
        let qualified_definition = definition_for(&mut server, &main, source, "out1 = mode");
        let bare_definition = definition_for(&mut server, &main, source, "out2 = mode");
        let qualified_hover =
            hover_markdown_for(&mut server, &main, source, "out1 = mode").unwrap_or_default();

        assert_ne!(
            qualified_definition,
            json!(null),
            "qualified namespace segment should resolve"
        );
        assert_eq!(
            qualified_definition["range"]["start"]["line"],
            json!(0),
            "qualified segment should resolve to the namespace, not outer::mode const"
        );
        assert_ne!(
            bare_definition,
            json!(null),
            "bare param reference should resolve"
        );
        assert_eq!(bare_definition["range"]["start"]["line"], json!(8));
        assert!(
            qualified_hover.contains("namespace mode"),
            "hover: {qualified_hover}"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_runtime_scope_consts() {
        let dir = mk_temp_dir("definition_runtime_const");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

sample:
  const Bias = 0.5
  out1 = Bias
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "Bias");

        assert_ne!(definition, json!(null), "runtime const should resolve");
        assert_eq!(definition["range"]["start"]["line"], json!(5));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_returns_null_for_explicit_use_ambiguity() {
        let dir = mk_temp_dir("definition_use_ambiguity");
        let main = dir.join("main.onda");
        let source = r#"
import std/math
use std::math

outs:
  out1

def clamp(x, lo, hi):
  return x

sample:
  out1 = clamp(2.0, 0.0, 1.0)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "out1 = clamp");

        assert_eq!(
            definition,
            json!(null),
            "ambiguous use collision should not jump to one arbitrary target"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_honors_imported_use_privacy() {
        let dir = mk_temp_dir("definition_use_privacy");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        write_file(
            &lib,
            r#"
namespace helpers:
  def shape(x):
    return x

use helpers

def shaped(x):
  return shape(x)
"#,
        );
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = shaped(0.0)
  out1 = shape(0.0)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let shaped = definition_for(&mut server, &main, source, "shaped");
        let shape = definition_for(&mut server, &main, source, "shape");

        assert_ne!(shaped, json!(null), "imported public def should resolve");
        assert_eq!(
            shape,
            json!(null),
            "private imported use should not resolve in importer"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_reexports_pub_use_symbols() {
        let dir = mk_temp_dir("definition_pub_use");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        write_file(
            &lib,
            r#"
namespace helpers:
  def shape(x):
    return x

pub use helpers
"#,
        );
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = shape(0.0)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "shape");

        assert_ne!(definition, json!(null), "pub use should resolve");
        assert!(definition["uri"]
            .as_str()
            .unwrap_or_default()
            .contains("lib.onda"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_namespace_local_relative_use() {
        let dir = mk_temp_dir("definition_namespace_local_relative_use");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP:
  namespace helpers:
    def shape(x):
      return x

  use helpers

  def run(x):
    return shape(x)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "return shape");

        assert_ne!(
            definition,
            json!(null),
            "namespace-local relative use should resolve"
        );
        assert_eq!(definition["range"]["start"]["line"], json!(3));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_namespace_local_use_does_not_leak() {
        let dir = mk_temp_dir("definition_namespace_local_use_leak");
        let main = dir.join("main.onda");
        let source = r#"
namespace A:
  namespace helpers:
    def hidden(x):
      return x

  use helpers

  def run(x):
    return hidden(x)

namespace B:
  def run(x):
    return hidden(x)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "return hidden");

        assert_eq!(
            definition,
            json!(null),
            "namespace-local use should not resolve outside its namespace"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_imported_defs() {
        let dir = mk_temp_dir("definition_imported_def");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        write_file(
            &lib,
            r#"
def shaped(x, amount = 1.0):
  return x * amount
"#,
        );
        let source = r#"
import lib

outs:
  out1

sample:
  out1 = shaped(0.0, amount = 0.5)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "shaped");
        let arg_definition = definition_for(&mut server, &main, source, "amount");

        assert_ne!(definition, json!(null), "imported def should resolve");
        assert!(definition["uri"]
            .as_str()
            .unwrap_or_default()
            .contains("lib.onda"));
        assert_eq!(definition["range"]["start"]["line"], json!(1));
        assert_ne!(
            arg_definition,
            json!(null),
            "imported def named argument should resolve"
        );
        assert!(arg_definition["uri"]
            .as_str()
            .unwrap_or_default()
            .contains("lib.onda"));
        assert_eq!(arg_definition["range"]["start"]["line"], json!(1));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_imported_proc_constructors_and_events() {
        let dir = mk_temp_dir("definition_imported_proc_event");
        let lib = dir.join("lib.onda");
        let main = dir.join("main.onda");
        write_file(
            &lib,
            r#"
proc Voice:
  params:
    gain = 1.0

  outs:
    out1

  event set(v):
    gain = v

  sample:
    out1 = gain
"#,
        );
        let source = r#"
import lib

outs:
  out1

init:
  voice = Voice()

sample:
  voice.set(v = 0.5)
  out1 = voice()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let proc_def = definition_for(&mut server, &main, source, "Voice");
        let event_def = definition_for(&mut server, &main, source, "set");
        let event_arg = definition_for(&mut server, &main, source, "set(v");

        assert_ne!(proc_def, json!(null), "imported proc should resolve");
        assert!(proc_def["uri"]
            .as_str()
            .unwrap_or_default()
            .contains("lib.onda"));
        assert_eq!(proc_def["range"]["start"]["line"], json!(1));

        assert_ne!(event_def, json!(null), "imported proc event should resolve");
        assert!(event_def["uri"]
            .as_str()
            .unwrap_or_default()
            .contains("lib.onda"));
        assert_eq!(event_def["range"]["start"]["line"], json!(8));
        assert_ne!(
            event_arg,
            json!(null),
            "imported proc-event named argument should resolve"
        );
        assert!(event_arg["uri"]
            .as_str()
            .unwrap_or_default()
            .contains("lib.onda"));
        assert_eq!(event_arg["range"]["start"]["line"], json!(8));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_materializes_stdlib_goto_targets_readonly() {
        let dir = mk_temp_dir("definition_stdlib_materialized");
        let cache = dir.join("cache");
        let _env_lock = stdlib_cache_env_lock();
        let _guard = EnvVarGuard::set_path("ONDA_STDLIB_CACHE_DIR", &cache);
        let main = dir.join("main.onda");
        let source = r#"
import std/math

outs:
  out1

sample:
  out1 = std::math::clamp(0.5, 0.0, 1.0)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let import_target = definition_for(&mut server, &main, source, "std/math");
        let clamp_target = definition_for(&mut server, &main, source, "clamp");

        for target in [import_target, clamp_target] {
            assert_ne!(target, json!(null), "stdlib goto should resolve");
            let uri = target["uri"].as_str().expect("stdlib goto uri");
            let path = file_uri_to_path(uri).expect("stdlib target should be a file uri");
            assert!(
                path.starts_with(&cache),
                "stdlib target should be inside cache: {}",
                path.display()
            );
            assert_eq!(
                path.file_name().and_then(|name| name.to_str()),
                Some("math.onda")
            );
            assert!(path.exists(), "materialized stdlib file should exist");
            assert!(
                fs::metadata(&path)
                    .expect("materialized stdlib metadata")
                    .permissions()
                    .readonly(),
                "materialized stdlib file should be read-only"
            );
            assert_eq!(
                fs::read_to_string(&path).expect("read materialized stdlib"),
                onda_frontend::stdlib_module_source("std/math").expect("embedded std/math")
            );
        }

        drop(_guard);
        clear_readonly_recursive(&dir);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prelude_method_calls_support_hover_signature_help_and_definition() {
        let dir = mk_temp_dir("prelude_method_navigation");
        let cache = dir.join("cache");
        let _env_lock = stdlib_cache_env_lock();
        let _guard = EnvVarGuard::set_path("ONDA_STDLIB_CACHE_DIR", &cache);
        let main = dir.join("main.onda");
        let source = "buffers:\n  source: f32[]\nsample:\n  out1 = source.readL(0, 0.5)\n";
        write_file(&main, source);

        let mut server = LspServer::default();
        let hover = hover_markdown_for(&mut server, &main, source, "readL")
            .expect("prelude method hover should resolve");
        assert!(hover.contains("def readL("), "unexpected hover: {hover}");

        let signatures = request_with_position(
            &mut server,
            &main,
            source,
            "readL(0,",
            "textDocument/signatureHelp",
        );
        let labels = signatures["signatures"]
            .as_array()
            .expect("signature list")
            .iter()
            .filter_map(|signature| signature["label"].as_str())
            .collect::<Vec<_>>();
        assert!(
            labels.contains(&"def readL(buf, pos)"),
            "signatures: {labels:?}"
        );
        assert!(
            labels.contains(&"def readL(buf, ch: i32, pos)"),
            "signatures: {labels:?}"
        );
        assert_eq!(signatures["activeParameter"], json!(2));

        let definition = definition_for(&mut server, &main, source, "readL");
        assert_ne!(
            definition,
            json!(null),
            "prelude method goto should resolve"
        );
        let uri = definition["uri"].as_str().expect("stdlib definition uri");
        let path = file_uri_to_path(uri).expect("stdlib definition should be a file URI");
        assert!(
            path.starts_with(&cache),
            "unexpected target: {}",
            path.display()
        );
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("lookup.onda")
        );

        drop(_guard);
        clear_readonly_recursive(&dir);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_stdlib_proc_events_from_namespaced_generic_instances() {
        let dir = mk_temp_dir("definition_stdlib_proc_event_namespace");
        let cache = dir.join("cache");
        let _env_lock = stdlib_cache_env_lock();
        let _guard = EnvVarGuard::set_path("ONDA_STDLIB_CACHE_DIR", &cache);
        let main = dir.join("main.onda");
        let source = r#"
import std/convolution

proc Plain<T>:
  outs<T> 1
  sample:
    out1 = 0.0

init:
  conv = Plain<f32>()

namespace Test<FFTSize = 64, MaxImpulseLen = 1024>:
  proc Use<T>:
    ins<T> 1
    outs<T> 1

    init:
      conv = std::convolution<FFTSize, MaxImpulseLen>::BlockConvolver<T>()
      ir: T[MaxImpulseLen]

    sample:
      conv.set_impulse(ir)
      out1 = conv(in1)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "set_impulse");

        assert_ne!(
            definition,
            json!(null),
            "stdlib proc event should resolve from a namespaced proc instance"
        );
        let uri = definition["uri"].as_str().expect("stdlib event uri");
        let path = file_uri_to_path(uri).expect("stdlib event should be a file uri");
        assert!(
            path.starts_with(&cache),
            "stdlib event should materialize inside cache: {}",
            path.display()
        );
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("convolution.onda")
        );
        let line = definition["range"]["start"]["line"]
            .as_u64()
            .expect("event line") as usize;
        let materialized = fs::read_to_string(&path).expect("read materialized stdlib");
        let target_line = materialized
            .lines()
            .nth(line)
            .expect("definition line should exist");
        assert!(
            target_line.contains("set_impulse"),
            "definition should target set_impulse, got line: {target_line}"
        );

        drop(_guard);
        clear_readonly_recursive(&dir);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_materialized_stdlib_proc_state_inside_local_def() {
        let dir = mk_temp_dir("definition_materialized_stdlib_proc_state_local_def");
        let cache = dir.join("cache");
        let _env_lock = stdlib_cache_env_lock();
        let _guard = EnvVarGuard::set_path("ONDA_STDLIB_CACHE_DIR", &cache);
        let main = dir.join("main.onda");
        let source = r#"
import std/smoothing

init:
  lag = std::smoothing::Lag<f64>()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "Lag");
        let uri = definition["uri"].as_str().expect("stdlib proc uri");
        let smoothing_path = file_uri_to_path(uri).expect("stdlib proc should be a file uri");
        let smoothing_source =
            fs::read_to_string(&smoothing_path).expect("read materialized smoothing");

        let needle = "\n      coef";
        let state_definition =
            definition_for(&mut server, &smoothing_path, &smoothing_source, needle);
        assert_ne!(
            state_definition,
            json!(null),
            "{needle:?} should resolve inside materialized stdlib local def"
        );
        assert_eq!(
            state_definition["range"]["start"]["line"],
            json!(15),
            "{needle:?} should goto the init declaration: {state_definition:?}"
        );

        drop(_guard);
        clear_readonly_recursive(&dir);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_relative_qualified_paths_from_namespace_scope() {
        let dir = mk_temp_dir("definition_relative_qualified_namespace");
        let main = dir.join("main.onda");
        let source = r#"
namespace sc:
  namespace Convolution2<FFTSize = 64, MaxKernel = 1024>:
    namespace Mono:
      proc ar<T>:
        outs<T> 1
        sample:
          out1 = 0.0

  namespace Convolution3<FFTSize = 64, MaxKernel = 1024>:
    namespace Mono:
      proc ar<T>:
        init:
          conv = Convolution2<FFTSize, MaxKernel>::Mono::ar<T>()
        sample:
          out1 = conv()
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let mono = definition_for(
            &mut server,
            &main,
            source,
            "Convolution2<FFTSize, MaxKernel>::Mono",
        );
        let ar = definition_for(
            &mut server,
            &main,
            source,
            "Convolution2<FFTSize, MaxKernel>::Mono::ar",
        );

        assert_ne!(mono, json!(null), "qualified namespace should resolve");
        assert_eq!(
            mono["range"]["start"]["line"],
            json!(3),
            "Mono should resolve to Convolution2::Mono, not Convolution3::Mono"
        );
        assert_ne!(ar, json!(null), "qualified proc should resolve");
        assert_eq!(
            ar["range"]["start"]["line"],
            json!(4),
            "ar should resolve to Convolution2::Mono::ar, not Convolution3::Mono::ar"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_namespace_template_parameters_as_consts() {
        let dir = mk_temp_dir("definition_namespace_template_params");
        let main = dir.join("main.onda");
        let source = r#"
namespace Test<FFTSize = 64, MaxImpulseLen = 1024>:
  proc Use<T>:
    ins<T> 1
    outs<T> 1

    init:
      ir: T[MaxImpulseLen]

    sample:
      for i in 0..FFTSize:
        out1 = in1
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let max_impulse = definition_for(&mut server, &main, source, "T[MaxImpulseLen");
        let fft_size = definition_for(&mut server, &main, source, "0..FFTSize");

        for definition in [max_impulse, fft_size] {
            assert_ne!(
                definition,
                json!(null),
                "namespace template parameter should resolve"
            );
            assert_eq!(definition["range"]["start"]["line"], json!(1));
        }

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_receiver_members_and_hides_private_params() {
        let dir = mk_temp_dir("definition_receiver_members");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    private cutoff = 1000.0
    gain = 1.0

  outs:
    out1

  sample:
    out1 = gain

init:
  voice = Voice()

sample:
  out1 = voice.gain
  out1 = voice.cutoff
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let gain = definition_for(&mut server, &main, source, "gain");
        let cutoff = definition_for(&mut server, &main, source, "cutoff");

        assert_ne!(gain, json!(null), "public proc param should resolve");
        assert_eq!(gain["range"]["start"]["line"], json!(4));
        assert_eq!(
            cutoff,
            json!(null),
            "private proc param should not resolve through receiver access"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_does_not_leak_instances_from_other_scopes() {
        let dir = mk_temp_dir("definition_instance_scope_leak");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    gain = 1.0

  outs:
    out1

  sample:
    out1 = gain

def build():
  v = Voice()
  return 0.0

outs:
  out1

sample:
  out1 = v.gain
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let definition = definition_for(&mut server, &main, source, "v.gain");

        assert_eq!(
            definition,
            json!(null),
            "function-local instance should not leak into sample member definition"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_does_not_leak_control_flow_locals() {
        let dir = mk_temp_dir("definition_control_flow_scope");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

sample:
  if true:
    tmp = 1.0
  for i in 0..2:
    loop_tmp = f32(i)
  out1 = tmp + loop_tmp + f32(i)
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let branch_local = definition_for(&mut server, &main, source, "out1 = tmp");
        let loop_local = definition_for(&mut server, &main, source, "tmp + loop_tmp");
        let loop_var = definition_for(&mut server, &main, source, "f32(i");

        assert_eq!(
            branch_local,
            json!(null),
            "branch local should not resolve after branch"
        );
        assert_eq!(
            loop_local,
            json!(null),
            "loop body local should not resolve after loop"
        );
        assert_eq!(
            loop_var,
            json!(null),
            "loop variable should not resolve after loop"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_does_not_resolve_future_locals() {
        let dir = mk_temp_dir("definition_future_local");
        let main = dir.join("main.onda");
        let source = r#"
outs:
  out1

sample:
  earlier = 1.0
  out1 = earlier + later
  later = 2.0
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let earlier = definition_for(&mut server, &main, source, "out1 = earlier");
        let later = definition_for(&mut server, &main, source, "+ later");

        assert_ne!(earlier, json!(null), "earlier local should resolve");
        assert_eq!(
            later,
            json!(null),
            "future local should not resolve before declaration"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_top_level_params_and_init_state() {
        let dir = mk_temp_dir("definition_top_level_runtime_scope");
        let main = dir.join("main.onda");
        let source = r#"
params:
  gain = 1.0

outs:
  out1

init:
  phase = 0.0

sample:
  out1 = gain + phase
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let gain = definition_for(&mut server, &main, source, "gain");
        let phase = definition_for(&mut server, &main, source, "phase");

        assert_ne!(gain, json!(null), "top-level param should resolve");
        assert_eq!(gain["range"]["start"]["line"], json!(2));
        assert_ne!(phase, json!(null), "top-level init state should resolve");
        assert_eq!(phase["range"]["start"]["line"], json!(8));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn definition_resolves_proc_params_and_init_state() {
        let dir = mk_temp_dir("definition_proc_runtime_scope");
        let main = dir.join("main.onda");
        let source = r#"
proc Voice:
  params:
    gain = 1.0

  outs:
    out1

  init:
    cached = 0.0

  sample:
    out1 = gain + cached
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let gain = definition_for(&mut server, &main, source, "gain");
        let cached = definition_for(&mut server, &main, source, "cached");

        assert_ne!(gain, json!(null), "proc param should resolve");
        assert_eq!(gain["range"]["start"]["line"], json!(3));
        assert_ne!(cached, json!(null), "proc init state should resolve");
        assert_eq!(cached["range"]["start"]["line"], json!(9));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn document_symbols_include_nested_namespace_items() {
        let dir = mk_temp_dir("document_symbols_namespace");
        let main = dir.join("main.onda");
        let source = r#"
namespace DSP:
  const Bias = 0.5

  struct Shape:
    x: f32

  def shape(x):
    return x
"#;
        write_file(&main, source);

        let mut server = LspServer::default();
        let symbols = document_symbols_for(&mut server, &main, source);
        let dsp = symbols
            .iter()
            .find(|symbol| symbol["name"] == json!("DSP"))
            .expect("DSP namespace symbol");
        let child_names = dsp["children"]
            .as_array()
            .expect("namespace children")
            .iter()
            .filter_map(|symbol| symbol["name"].as_str())
            .collect::<Vec<_>>();

        assert!(child_names.contains(&"Bias"), "children: {child_names:?}");
        assert!(child_names.contains(&"Shape"), "children: {child_names:?}");
        assert!(child_names.contains(&"shape"), "children: {child_names:?}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn protocol_completion_lists_virtual_project_imports() {
        let mut session = LspSession::new();
        session
            .handle_message(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "processId": null, "capabilities": {} },
            }))
            .expect("initialize should succeed");
        for (uri, text) in [
            (
                "file:///onda-project/lib.onda",
                "namespace Lib:\n  const value = 1.0\n",
            ),
            ("file:///onda-project/main.onda", "import l\n"),
        ] {
            session
                .handle_message(json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": {
                        "textDocument": {
                            "uri": uri,
                            "languageId": "onda",
                            "version": 1,
                            "text": text,
                        }
                    },
                }))
                .expect("virtual document should open");
        }

        let messages = session
            .handle_message(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/completion",
                "params": {
                    "textDocument": { "uri": "file:///onda-project/main.onda" },
                    "position": { "line": 0, "character": 8 },
                },
            }))
            .expect("completion should succeed");
        let items = messages[0]["result"]["items"]
            .as_array()
            .expect("completion items");
        assert!(
            items.iter().any(|item| item["label"] == json!("lib")),
            "virtual project module should be offered: {items:?}"
        );
    }

    #[test]
    fn protocol_serves_read_only_stdlib_virtual_documents() {
        let mut session = LspSession::new();
        let messages = session
            .handle_message(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "onda/virtualDocument",
                "params": { "uri": "onda-stdlib:///std/osc.onda" },
            }))
            .expect("virtual document request should succeed");
        let document = &messages[0]["result"];
        assert_eq!(document["path"], json!("std/osc.onda"));
        assert_eq!(document["languageId"], json!("onda"));
        assert_eq!(document["readOnly"], json!(true));
        assert!(
            document["text"]
                .as_str()
                .is_some_and(|source| source.contains("proc Saw")),
            "virtual document should contain the embedded std/osc source"
        );

        let source = document["text"].as_str().expect("stdlib source").to_owned();
        let position = position_after(&source, "Phasor");
        let messages = session
            .handle_message(json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "textDocument/definition",
                "params": {
                    "textDocument": { "uri": "onda-stdlib:///std/osc.onda" },
                    "position": {
                        "line": position.line,
                        "character": position.character,
                    }
                },
            }))
            .expect("virtual stdlib definition request should succeed");
        let definition = &messages[0]["result"];
        assert_eq!(definition["uri"], json!("onda-stdlib:///std/osc.onda"));
        assert_eq!(definition["range"]["start"]["line"], json!(1));
    }
