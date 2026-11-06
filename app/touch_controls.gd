class_name TouchControls
## Touch and Pencil for Godot controls that only understand the mouse. AirDraw doesn't turn
## touches into mouse events (the canvas reads fingers and the Pencil itself), and of Godot's
## controls only buttons handle touch; sliders and scroll containers need these.


## Lets a finger or the Pencil drag `slider` (horizontal), like a mouse would: drag_started,
## value_changed while moving, drag_ended.
static func slider(s: HSlider) -> void:
	if s.has_meta("touch"):
		return
	s.set_meta("touch", true)
	s.gui_input.connect(func(e: InputEvent) -> void:
		if e is InputEventScreenTouch and e.index == 0:
			if e.pressed:
				s.drag_started.emit()
				s.ratio = _slider_ratio(s, e.position)
			else:
				s.drag_ended.emit(true)
			s.accept_event()
		elif e is InputEventScreenDrag and e.index == 0:
			s.ratio = _slider_ratio(s, e.position)
			s.accept_event())


## Where along the track `at` is, 0–1 (`ratio` also maps log-scale sliders).
static func _slider_ratio(s: HSlider, at: Vector2) -> float:
	var grabber := s.get_theme_icon("grabber").get_width() if s.has_theme_icon("grabber") else 0
	return clampf((at.x - grabber * 0.5) / maxf(s.size.x - grabber, 1.0), 0.0, 1.0)


## Lets a finger scroll `container` by dragging anywhere in it, also over buttons: once the
## finger has moved a little it scrolls, and the button it started on isn't pressed.
static func scroll(container: ScrollContainer) -> void:
	if container.has_node("TouchScroll"):
		return
	var n := TouchScroll.new()
	n.name = "TouchScroll"
	container.add_child(n)


class TouchScroll extends Node:
	## Finger travel (pixels at the design scale) before a press becomes a scroll.
	const SLOP := 10.0
	var _index := -1
	var _start := Vector2.ZERO
	var _scrolling := false
	var _injecting := false

	func _input(event: InputEvent) -> void:
		var c := get_parent() as ScrollContainer
		if _injecting or c == null or not c.is_visible_in_tree():
			return
		if event is InputEventScreenTouch:
			if event.pressed and _index < 0 and c.get_global_rect().has_point(event.position):
				_index = event.index
				_start = event.position
				_scrolling = false
			elif not event.pressed and event.index == _index:
				_index = -1
				_scrolling = false
		elif event is InputEventScreenDrag and event.index == _index:
			if not _scrolling and event.position.distance_to(_start) > SLOP * c.get_viewport().get_visible_rect().size.y / 900.0:
				_scrolling = true
				# A button decides on release from the last drag it saw: tell the one under the
				# finger that the finger left it, so lifting doesn't press it.
				var away := InputEventScreenDrag.new()
				away.index = event.index
				away.position = Vector2(-1e6, -1e6)
				_injecting = true
				get_viewport().push_input(away)
				_injecting = false
			if _scrolling:
				c.scroll_vertical -= int(event.relative.y)
				if c.horizontal_scroll_mode != ScrollContainer.SCROLL_MODE_DISABLED:
					c.scroll_horizontal -= int(event.relative.x)
				get_viewport().set_input_as_handled()
