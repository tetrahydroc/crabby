extends Node
class_name Example

var counter := 0

func _ready():
	counter = 1

func tick(delta) -> int:
	counter += 1
	return counter

static func util() -> int:
	return 42
