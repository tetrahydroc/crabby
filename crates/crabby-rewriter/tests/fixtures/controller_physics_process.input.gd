extends CharacterBody3D
class_name Controller

var walk_speed := 5.0

func _physics_process(delta):
	velocity += Vector3(0, -9.8, 0) * delta
	move_and_slide()

func _input(event):
	pass
