extends Control

var fade = false

func _ready() -> void:
	fade = true
	await get_tree().create_timer(5.0).timeout
	fade = false

func update_fade(delta: float):
	fade = delta > 0.0
