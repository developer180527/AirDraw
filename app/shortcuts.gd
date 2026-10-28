class_name Shortcuts
## Every keyboard shortcut in one table: key handling and tooltips both read it, so they
## can't drift apart (docs/ARCHITECTURE.md §5-3).

## action → [keycode, needs Cmd/Ctrl, needs Shift]
const KEYS := {
	"tool_draw": [KEY_B, false, false],
	"tool_erase": [KEY_E, false, false],
	"tool_select": [KEY_V, false, false],
	"select_all": [KEY_A, true, false],
	"deselect": [KEY_ESCAPE, false, false],
	"duplicate": [KEY_D, true, false],
	"delete_selection": [KEY_BACKSPACE, false, false],
	"rebrush_selection": [KEY_R, false, true],
	"tool_guide": [KEY_G, false, false],
	"undo": [KEY_Z, true, false],
	"redo": [KEY_Z, true, true],
	"save": [KEY_S, true, false],
	"export": [KEY_E, true, false],
	"clear": [KEY_BACKSPACE, true, false],
	"view_front": [KEY_F, false, false],
	"view_side": [KEY_S, false, false],
	"view_top": [KEY_T, false, false],
	"view_reset": [KEY_0, false, false],
	"toggle_ui": [KEY_TAB, false, false],
	"rebrush": [KEY_R, false, false],
	"brush_1": [KEY_1, false, false],
	"brush_2": [KEY_2, false, false],
	"brush_3": [KEY_3, false, false],
	"brush_4": [KEY_4, false, false],
	"brush_5": [KEY_5, false, false],
	"brush_6": [KEY_6, false, false],
	"brush_7": [KEY_7, false, false],
	"brush_8": [KEY_8, false, false],
	"brush_9": [KEY_9, false, false],
}


## The action a key press triggers, or "".
static func action_for(event: InputEventKey) -> String:
	var cmd := event.is_command_or_control_pressed()
	for action in KEYS:
		var k: Array = KEYS[action]
		if event.keycode == k[0] and cmd == k[1] and event.shift_pressed == k[2]:
			return action
	return ""


## How the shortcut reads on this platform: "G", "⌘Z", "Ctrl+Shift+Z".
static func hint(action: String) -> String:
	if not KEYS.has(action):
		return ""
	var k: Array = KEYS[action]
	var apple := OS.has_feature("macos") or OS.has_feature("ios")
	var text := OS.get_keycode_string(k[0])
	if k[0] == KEY_BACKSPACE:
		text = "⌫" if apple else "Backspace"
	elif k[0] == KEY_ESCAPE:
		text = "Esc"
	if k[2]:
		text = ("⇧" if apple else "Shift+") + text
	if k[1]:
		text = ("⌘" if apple else "Ctrl+") + text
	return text


## A tooltip with the shortcut, e.g. "Guide (G)". Touch-only devices get the label alone.
static func tooltip(label: String, action: String) -> String:
	var h := hint(action)
	if h.is_empty() or not (OS.has_feature("pc") or DisplayServer.has_hardware_keyboard()):
		return label
	return "%s (%s)" % [label, h]
