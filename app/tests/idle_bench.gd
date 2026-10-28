extends Node
## How much work the app does when nothing changes (race-to-idle, docs/ARCHITECTURE.md §10):
## frames drawn per second, CPU and GPU time per frame and draw calls, in phases: the home
## screen, an open sketch left alone, a slow orbit, and the sketch left alone again. Added by
## main.gd for the `bench-idle` launch argument, so it measures the real app. Keeps the
## report in user://idle_bench.txt. iPad: BENCH=idle scripts/bench_ipad.sh

const PHASE := 8.0
const SETTLE := 3.0

var main: Node


func _ready() -> void:
	if "low-processor" in OS.get_cmdline_user_args():
		OS.low_processor_usage_mode = true
	var vp := get_viewport().get_viewport_rid()
	RenderingServer.viewport_set_measure_render_time(vp, true)
	var report := "BENCH idle os=%s viewport=%s max_fps=%d vsync=%d low_processor=%s\n" % [
		OS.get_name(), get_viewport().get_visible_rect().size, Engine.max_fps,
		DisplayServer.window_get_vsync_mode(), OS.low_processor_usage_mode]
	report += "phase            start_s  frames/s  process/s  cpu_ms  gpu_ms  draw_calls\n"
	await _wait(SETTLE)
	report += await _phase("home")
	var recents: Array = main.get("app_files").recents()
	if not recents.is_empty():
		main.get("documents").open_recent(recents[0])
	await _wait(SETTLE)
	report += await _phase("canvas idle")
	report += await _phase("canvas orbit", func() -> void: main.get("rig").orbit(Vector2(3, 0)))
	await _wait(SETTLE)
	report += await _phase("canvas idle")
	print(report)
	var f := FileAccess.open("user://idle_bench.txt", FileAccess.WRITE)
	f.store_string(report)
	f.close()
	if not OS.has_feature("ios"):
		get_tree().quit()


func _wait(seconds: float) -> void:
	await get_tree().create_timer(seconds).timeout


## Averages over PHASE seconds of real time, calling `each_frame` every processed frame.
func _phase(name: String, each_frame := Callable()) -> String:
	var vp := get_viewport().get_viewport_rid()
	var start := Time.get_ticks_usec()
	var drawn0 := Engine.get_frames_drawn()
	var processed0 := Engine.get_process_frames()
	var cpu := 0.0
	var gpu := 0.0
	var calls := 0
	var samples := 0
	while Time.get_ticks_usec() - start < PHASE * 1e6:
		if each_frame.is_valid():
			each_frame.call()
		await get_tree().process_frame
		cpu += RenderingServer.viewport_get_measured_render_time_cpu(vp) + RenderingServer.get_frame_setup_time_cpu()
		gpu += RenderingServer.viewport_get_measured_render_time_gpu(vp)
		calls += RenderingServer.get_rendering_info(RenderingServer.RENDERING_INFO_TOTAL_DRAW_CALLS_IN_FRAME)
		samples += 1
	var secs := (Time.get_ticks_usec() - start) / 1e6
	var n := maxi(samples, 1)
	return "%-16s %7.1f  %8.1f  %9.1f  %6.2f  %6.2f  %10d\n" % [name, start / 1e6, (Engine.get_frames_drawn() - drawn0) / secs,
		(Engine.get_process_frames() - processed0) / secs, cpu / n, gpu / n, calls / n]
