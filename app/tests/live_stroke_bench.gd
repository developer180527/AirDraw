extends Node

## Measures what a live stroke costs per update as it grows (design 3e): stream build,
## conversion to Godot arrays and mesh upload. Feeds a 20 s spiral at 240 Hz, 2 samples per
## frame (Pencil at 120 Hz), straight into the canvas: no input path. Then checks that the
## live stroke, the finished stroke and the stroke rebuilt from the document look identical.
## Prints a summary (also kept in user://live_stroke_bench.txt, since print doesn't reach the
## host from iOS) and writes every update, plus frame and render times (display-paced; no GPU
## timing on Metal), to user://live_stroke_bench.csv.
## Run: godot --path app res://tests/live_stroke_bench.tscn
## `++ long` instead runs one ~2.6M-triangle stroke that hits the 256-surface merge fallback.

const SAMPLE_HZ := 240.0
const PER_FRAME := 2
const SECONDS := 20

var canvas: SketchCanvas
var summary: FileAccess


func _ready() -> void:
	summary = FileAccess.open("user://live_stroke_bench.txt", FileAccess.WRITE)
	var env := WorldEnvironment.new()
	env.environment = Environment.new()
	env.environment.background_mode = Environment.BG_COLOR
	env.environment.background_color = Color(0.93, 0.93, 0.91)
	env.environment.ambient_light_source = Environment.AMBIENT_SOURCE_COLOR
	env.environment.ambient_light_color = Color(0.75, 0.75, 0.8)
	add_child(env)
	var sun := DirectionalLight3D.new()
	sun.rotation = Vector3(-0.9, 0.5, 0.0)
	sun.light_energy = 0.8
	add_child(sun)
	# The spiral reaches radius 2 at 20 s; keep all of it on screen so it's really drawn.
	var cam := Camera3D.new()
	cam.position = Vector3(0, 0, 4.5)
	add_child(cam)
	canvas = SketchCanvas.new()
	add_child(canvas)
	RenderingServer.viewport_set_measure_render_time(get_viewport().get_viewport_rid(), true)

	var csv := FileAccess.open("user://live_stroke_bench.csv", FileAccess.WRITE)
	csv.store_line("brush,update,points,vertices,uploaded_vertices,build_ms,convert_ms,upload_ms,frame_ms,render_cpu_ms,render_gpu_ms")
	_out("BENCH os=%s renderer=%s gpu=%s" % [OS.get_name(), RenderingServer.get_current_rendering_driver_name(),
		RenderingServer.get_video_adapter_name()])
	if "long" in OS.get_cmdline_user_args():
		await _long_run()
		summary.close()
		get_tree().quit()
		return
	for id in ["com.airdraw.ink", "com.airdraw.ribbon"]:
		await _run(id, csv)
	csv.close()
	_out("BENCH csv=" + ProjectSettings.globalize_path("user://live_stroke_bench.csv"))
	summary.close()
	get_tree().quit()


func _run(brush_id: String, csv: FileAccess) -> void:
	canvas.clear()
	canvas.set_brush(brush_id)
	for i in 30:  # let shaders compile and frame times settle
		await get_tree().process_frame
	var vp := get_viewport().get_viewport_rid()
	var frames := PackedFloat64Array()
	var render_cpu := PackedFloat64Array()
	var render_gpu := PackedFloat64Array()
	canvas.set_record_live_timings(true)
	canvas.begin_stroke(_spiral(0), Vector3.BACK, 0.7, 0.0)
	var n := 1
	var last := Time.get_ticks_usec()
	while n < int(SAMPLE_HZ) * SECONDS:
		var pos := PackedVector3Array()
		var pressure := PackedFloat32Array()
		var times := PackedFloat64Array()
		for k in PER_FRAME:
			pos.append(_spiral(n))
			pressure.append(0.7)
			times.append(n / SAMPLE_HZ)
			n += 1
		canvas.add_points(pos, Vector3.BACK, pressure, times)
		await get_tree().process_frame
		var now := Time.get_ticks_usec()
		frames.append((now - last) / 1000.0)
		render_cpu.append(RenderingServer.viewport_get_measured_render_time_cpu(vp))
		render_gpu.append(RenderingServer.viewport_get_measured_render_time_gpu(vp))
		last = now
	# What the live stroke shows (sealed chunks + open chunk + tail) must match the single
	# mesh it becomes at pen-up.
	await RenderingServer.frame_post_draw
	var live_img := get_viewport().get_texture().get_image()
	var t0 := Time.get_ticks_usec()
	canvas.end_stroke()
	var end_ms := (Time.get_ticks_usec() - t0) / 1000.0
	await get_tree().process_frame
	await RenderingServer.frame_post_draw
	var final_img := get_viewport().get_texture().get_image()
	var diff := _diff(live_img, final_img)
	# The finished node (sealed chunks as surfaces) against the node the document rebuilds
	# from the raw points (undo + redo): same triangles, same pixels.
	var chunked := _stroke_triangles()
	canvas.undo()
	canvas.redo()
	await get_tree().process_frame
	await RenderingServer.frame_post_draw
	var rebuilt_img := get_viewport().get_texture().get_image()
	var rebuilt_diff := _diff(final_img, rebuilt_img)
	if rebuilt_diff.x > 0:
		final_img.save_png("user://bench_%s_finished.png" % brush_id)
		rebuilt_img.save_png("user://bench_%s_rebuilt.png" % brush_id)
	var rebuilt := _stroke_triangles()
	var t: Dictionary = canvas.take_live_timings()
	canvas.set_record_live_timings(false)

	var updates: int = frames.size()
	var total := PackedFloat64Array()
	for i in updates:
		total.append(t.build_ms[i] + t.convert_ms[i] + t.upload_ms[i])
	for i in updates:
		csv.store_line("%s,%d,%d,%d,%d,%.4f,%.4f,%.4f,%.3f,%.3f,%.3f" % [brush_id, i, t.points[i], t.vertices[i], t.uploaded_vertices[i],
			t.build_ms[i], t.convert_ms[i], t.upload_ms[i], frames[i], render_cpu[i], render_gpu[i]])
	_out("BENCH brush=%s  (per second of stroke: medians, max in brackets)" % brush_id)
	_out("BENCH   sec  points  vertices  uploaded  build_ms  convert_ms  upload_ms          total_ms")
	var per_sec := int(SAMPLE_HZ) / PER_FRAME
	for s: int in [1, 2, 5, 10, 15, 20]:
		var a := (s - 1) * per_sec
		var b := mini(s * per_sec, updates)
		_out("BENCH   %3d  %6d  %8d  %8d  %8.3f  %10.3f  %9.3f  %8.3f [%5.2f]" % [s, t.points[b - 1],
			t.vertices[b - 1], int(_max(t.uploaded_vertices, a, b)), _median(t.build_ms, a, b), _median(t.convert_ms, a, b),
			_median(t.upload_ms, a, b), _median(total, a, b), _max(total, a, b)])
	_out("BENCH   end_stroke_ms=%.2f (finish: build %.2f, convert %.2f, upload %.2f)" % [
		end_ms, t.build_ms[updates], t.convert_ms[updates], t.upload_ms[updates]])
	_out("BENCH   live vs finished: %d of %d stroke pixels differ; finished (%d surfaces, %d triangles) vs rebuilt (%d surfaces, %d triangles): %d pixels differ" % [
		diff.x, diff.y, chunked.x, chunked.y, rebuilt.x, rebuilt.y, rebuilt_diff.x])


## A stroke long enough to hit Godot's 256-surface limit and the merge fallback, fed 25 points
## per update. Checks the finished node against the rebuilt one.
func _long_run() -> void:
	canvas.clear()
	canvas.set_brush("com.airdraw.ink")
	canvas.begin_stroke(_spiral(0), Vector3.BACK, 0.7, 0.0)
	var n := 1
	while n < 55000:
		var pos := PackedVector3Array()
		var pressure := PackedFloat32Array()
		var times := PackedFloat64Array()
		for k in 25:
			pos.append(_spiral(n))
			pressure.append(0.7)
			times.append(n / SAMPLE_HZ)
			n += 1
		canvas.add_points(pos, Vector3.BACK, pressure, times)
		await get_tree().process_frame
	canvas.end_stroke()
	var chunked := _stroke_triangles()
	canvas.undo()
	canvas.redo()
	var rebuilt := _stroke_triangles()
	_out("BENCH long stroke: finished %d surfaces, %d triangles; rebuilt %d surfaces, %d triangles" % [chunked.x, chunked.y, rebuilt.x, rebuilt.y])


## (surfaces, triangles) of the newest stroke node.
func _stroke_triangles() -> Vector2i:
	var node: MeshInstance3D = null
	for c in canvas.get_children():
		if c is MeshInstance3D and not c.is_queued_for_deletion():
			node = c
	var mesh := node.mesh
	var tris := 0
	for i in mesh.get_surface_count():
		tris += mesh.surface_get_array_index_len(i) / 3
	return Vector2i(mesh.get_surface_count(), tris)


## (pixels that differ, stroke pixels in `a`).
func _diff(a: Image, b: Image) -> Vector2i:
	var differing := 0
	var drawn := 0
	for y in a.get_height():
		for x in a.get_width():
			var p := a.get_pixel(x, y)
			if not p.is_equal_approx(b.get_pixel(x, y)):
				differing += 1
			if p.r < 0.9:
				drawn += 1
	return Vector2i(differing, drawn)


func _spiral(i: int) -> Vector3:
	var t := i / SAMPLE_HZ
	return Vector3(cos(t) * t * 0.1, sin(t) * t * 0.1, 0.0)


func _median(a, from: int, to: int) -> float:
	var s: Array = Array(a.slice(from, to))
	s.sort()
	return s[s.size() / 2]


func _max(a, from: int, to: int) -> float:
	var m := 0.0
	for i in range(from, to):
		m = maxf(m, a[i])
	return m


## Prints and keeps the line in user://live_stroke_bench.txt (print doesn't reach the host from iOS).
func _out(line: String) -> void:
	print(line)
	summary.store_line(line)
