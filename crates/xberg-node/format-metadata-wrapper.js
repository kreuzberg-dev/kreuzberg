// Wrap JsFormatMetadata to add getters for format-specific metadata
// This works around the limitation that #[napi(getter)] doesn't work on
// #[napi(object)]

export function wrapFormatMetadata(fmt) {
  if (!fmt || typeof fmt !== "object") return fmt;

  const tag = fmt.format_type;
  const payload = fmt["0"];

  if (!payload) return fmt;

  try {
    const data = JSON.parse(payload);

    // Add the typed variant property as a non-enumerable property
    Object.defineProperty(fmt, tag, {
      value: data,
      enumerable: false,
      writable: false,
      configurable: false,
    });
  } catch (e) {
    // Ignore JSON parse errors
  }

  return fmt;
}
