extends Node

## Erase timings on this device (docs/ARCHITECTURE.md §3i): eraser picking on 100k- and
## 1M-point sketches, lasso and tap, and partial and whole erases of a 5,000-point stroke with
## its live pieces rebuilt. Keeps the report in user://erase_bench.txt (print doesn't reach
## the host from iOS).
## Run: godot --path app res://tests/erase_bench.tscn  (iPad: BENCH=erase scripts/bench_ipad.sh)


func _ready() -> void:
	var cam := Camera3D.new()
	cam.position = Vector3(0, 0, 0)
	add_child(cam)
	var canvas := SketchCanvas.new()
	add_child(canvas)
	for i in 3:
		await get_tree().process_frame
	var size := get_viewport().get_visible_rect().size
	var report := "BENCH erase os=%s viewport=%dx%d\n%s" % [OS.get_name(), size.x, size.y, canvas.erase_benchmark(cam)]
	print(report)
	var f := FileAccess.open("user://erase_bench.txt", FileAccess.WRITE)
	f.store_string(report + "\n")
	f.close()
	if not OS.has_feature("ios"):
		get_tree().quit()
