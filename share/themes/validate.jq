# A theme is data, never a shell program.
def valid_theme:
  type == "object" and all(to_entries[];
    (.value | type == "string") and
    (if (.key | test("^SHOWY_QUOTA_PALETTE_(PRIMARY_(GOOD|WARN|BAD|UNKNOWN)|DIM_(GOOD|WARN|BAD|UNKNOWN)|BG|SURFACE|TRACK|ICON_TEXT|COUNTDOWN|COUNTDOWN_WARN|STALE|ELAPSED|ELAPSED_LONG)$"))
     then (.value | test("^[0-9a-fA-F]{6}$"))
     elif .key == "SHOWY_QUOTA_PALETTE_DIM_SCALE"
     then (.value | test("^(0(\\.[0-9]+)?|1(\\.0+)?)$"))
     elif .key == "SHOWY_QUOTA_SEVERITY_GLYPHS"
     then (.value | test("^(on|off|true|false|1|0)$"))
     elif (.key | test("^SHOWY_QUOTA_(CAP_LEFT|CAP_RIGHT|STALE_GLYPH|DEGRADED_CLI_GLYPH|ERROR_GLYPH)$"))
     then (.value | length <= 16 and (test("[\u0000-\u001f\u007f-\u009f]") | not))
     else false end));
