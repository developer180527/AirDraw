class_name Icons
## App icons as inline SVG (24×24, white strokes), rendered at the UI scale on demand and
## tinted by the theme. No import step, sharp at any size.

const _PATHS := {
	"back": '<path d="M15 5 8 12l7 7"/>',
	"undo": '<path d="M9 14 4 9l5-5"/><path d="M4 9h11a5 5 0 0 1 0 10h-4"/>',
	"redo": '<path d="m15 14 5-5-5-5"/><path d="M20 9H9a5 5 0 0 0 0 10h4"/>',
	"export": '<path d="M12 15V3M7 8l5-5 5 5"/><path d="M5 13v7h14v-7"/>',
	"more": '<circle cx="5" cy="12" r="1.4" fill="white"/><circle cx="12" cy="12" r="1.4" fill="white"/><circle cx="19" cy="12" r="1.4" fill="white"/>',
	"brush": '<path d="M4 20l1-4L16 5l3 3L8 19z"/><path d="M14 7l3 3"/>',
	"eraser": '<path d="M7 20h13"/><path d="M4 15 14 5l5 5-8 8H7z"/><path d="m9 10 5 5"/>',
	"select": '<path d="M4 4h4M12 4h4M20 4v4M20 12v4M20 20h-4M12 20H8M4 20v-4M4 12V8" />',
	"plane": '<path d="M3 16 9 8h12l-6 8z"/>',
	"guide": '<path d="M4 17c3-7 7-9 16-9"/><path d="M4 17v3M20 8v3" stroke-dasharray="1.5 1.5"/><path d="M4 20c3-7 7-9 16-9" opacity="0.5"/>',
	"sphere": '<circle cx="12" cy="12" r="8"/><ellipse cx="12" cy="12" rx="8" ry="3"/>',
	"cylinder": '<ellipse cx="12" cy="6" rx="6" ry="2.5"/><path d="M6 6v12a6 2.5 0 0 0 12 0V6"/>',
	"symmetry": '<path d="M12 3v18" stroke-dasharray="2 2"/><path d="M9 7 4 12l5 5zM15 7l5 5-5 5z"/>',
	"snap": '<circle cx="12" cy="12" r="7"/><path d="M12 2v5M12 17v5M2 12h5M17 12h5"/>',
	"chevron_down": '<path d="m6 9 6 6 6-6"/>',
	"chevron_up": '<path d="m6 15 6-6 6 6"/>',
}

static var _cache := {}


## Icon `name` rendered at 24 × `scale` pixels.
static func get_icon(name: String, scale: float) -> Texture2D:
	var key := "%s@%.2f" % [name, scale]
	if _cache.has(key):
		return _cache[key]
	var svg := '<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">%s</svg>' % _PATHS.get(name, "")
	var img := Image.new()
	img.load_svg_from_string(svg, scale)
	var tex := ImageTexture.create_from_image(img)
	_cache[key] = tex
	return tex
