(_
  "["
  "]" @end) @indent

(_
  "{"
  "}" @end) @indent

(_
  "("
  ")" @end) @indent

; Daml layout blocks
[
  (daml_fields)
  (daml_field_updates)
  (daml_field_patterns)
  (template_body)
  (interface_body)
  (interface_instance_body)
  (exception_body)
] @indent
