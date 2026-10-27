extends Node

## Measures what a live stroke costs per update as it grows (design 3e, step 0): brush build,
## conversion to Godot arrays, mesh upload, and the whole frame. Feeds a 20 s spiral at
## 240 Hz, 2 samples per frame (Pencil at 120 Hz), straight into the canvas: no input path.
## Prints one line per second of stroke and writes every update (plus frame and render times,
## which are display-paced and, on Metal, have no GPU timing) to user://live_stroke_bench.csv.
## Run: godot --path app res://tests/live_stroke_bench.tscn

const SAMPLE_HZ := 240.0
const PER_FRAME := 2
const SECONDS := 20

var canvas: SketchCanvas


func _ready() -> void:
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
	csv.store_line("brush,update,points,vertices,build_ms,convert_ms,upload_ms,frame_ms,render_cpu_ms,render_gpu_ms")
	print("BENCH os=%s renderer=%s gpu=%s" % [OS.get_name(), RenderingServer.get_current_rendering_driver_name(),
		RenderingServer.get_video_adapter_name()])
	for id in ["com.airdraw.ink", "com.airdraw.ribbon"]:
		await _run(id, csv)
	csv.close()
	print("BENCH csv=", ProjectSettings.globalize_path("user://live_stroke_bench.csv"))
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
	var t0 := Time.get_ticks_usec()
	canvas.end_stroke()
	var end_ms := (Time.get_ticks_usec() - t0) / 1000.0
	var t: Dictionary = canvas.take_live_timings()
	canvas.set_record_live_timings(false)

	var updates: int = frames.size()
	var total := PackedFloat64Array()
	for i in updates:
		total.append(t.build_ms[i] + t.convert_ms[i] + t.upload_ms[i])
	for i in updates:
		csv.store_line("%s,%d,%d,%d,%.4f,%.4f,%.4f,%.3f,%.3f,%.3f" % [brush_id, i, t.points[i], t.vertices[i],
			t.build_ms[i], t.convert_ms[i], t.upload_ms[i], frames[i], render_cpu[i], render_gpu[i]])
	print("BENCH brush=%s  (per second of stroke: medians, max in brackets)" % brush_id)
	print("BENCH   sec  points  vertices  build_ms  convert_ms  upload_ms          total_ms")
	var per_sec := int(SAMPLE_HZ) / PER_FRAME
	for s: int in [1, 2, 5, 10, 15, 20]:
		var a := (s - 1) * per_sec
		var b := mini(s * per_sec, updates)
		print("BENCH   %3d  %6d  %8d  %8.3f  %10.3f  %9.3f  %8.3f [%5.2f]" % [s, t.points[b - 1],
			t.vertices[b - 1], _median(t.build_ms, a, b), _median(t.convert_ms, a, b),
			_median(t.upload_ms, a, b), _median(total, a, b), _max(total, a, b)])
	print("BENCH   end_stroke_ms=%.2f" % end_ms)


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
