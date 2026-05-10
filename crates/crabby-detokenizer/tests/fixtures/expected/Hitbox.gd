extends Node3D
class_name Hitbox

@export var type: String

func ApplyDamage(damage: float):
	var finalDamage = 0.0

	if type == "Head":
		finalDamage = 100.0


		if owner.activeVoice && is_instance_valid(owner.activeVoice):
			owner.activeVoice.queue_free()
			owner.activeVoice = null

	elif type == "Torso":
		finalDamage = damage
	elif type == "Leg_L" || type == "Leg_R":
		finalDamage = damage / 2.0

	owner.WeaponDamage(type, finalDamage)
