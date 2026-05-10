extends GPUParticles3D
class_name Flash

var duration := 0.05

func _ready():
	emitting = true

func _process(delta):
	duration -= delta
	if duration <= 0.0:
		queue_free()
