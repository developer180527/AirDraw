extends Node

## `.airdraw` format sizes and save/open times (docs/ARCHITECTURE.md §3h): every sketch in
## the Sketches folder, plus a 1M-point sketch tiled from them. The table comes from Rust
## (airdraw_doc::bench), the same code as the desktop `file_format_benchmark` test. Kept in
## user://file_format_bench.txt, since print doesn't reach the host from iOS.
## Run: godot --path app res://tests/file_format_bench.tscn   (iPad: scripts/bench_ipad.sh file-format)

const POINTS := 1_000_000


func _ready() -> void:
	var app_files := AppFiles.new()
	AppPaths.setup(app_files)
	var paths := PackedStringArray()
	for f in DirAccess.get_files_at(AppPaths.sketches()):
		if f.get_extension() == "airdraw":
			paths.append(AppPaths.sketches().path_join(f))
	app_files.free()
	var canvas := SketchCanvas.new()
	add_child(canvas)
	var report: String = "%s, %s\n%s" % [OS.get_name(), OS.get_processor_name(), canvas.file_format_benchmark(paths, POINTS)]
	print(report)
	var out := FileAccess.open("user://file_format_bench.txt", FileAccess.WRITE)
	out.store_string(report)
	out.close()
	if OS.get_name() != "iOS":
		get_tree().quit()
