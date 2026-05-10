extends Camera3D
class_name Camera


var gameData = preload("res://Resources/GameData.tres")

@export var camera: Camera3D
@export var attribute: CameraAttributesPractical

var translateSpeed: = 4.0
var rotateSpeed: = 4.0
var nearFarSpeed: = 1.0
var FOVSpeed: = 1.0
var interpolate = true
var currentRID: RID

func _ready():
	currentRID = get_tree().get_root().get_viewport_rid()

func _physics_process(delta):

	gameData.cameraPosition = global_position


	if gameData.isCaching: return


	if interpolate: Interpolate(delta)
	else: Follow()


	FOV(delta)
	DOF(delta)


	if camera.projection == projection:
		var nearFarFactor = nearFarSpeed * delta * 10
		var FOVFactor = FOVSpeed * delta * 10
		var newNear = lerp(near, camera.near, nearFarFactor) as float
		var newFar = lerp(far, camera.far, nearFarFactor) as float
		var newFOV = lerp(fov, camera.fov, FOVFactor) as float
		set_perspective(newFOV, newNear, newFar)

func Interpolate(delta):
	var translate_factor = translateSpeed * delta * 10
	var rotate_factor = rotateSpeed * delta * 10
	var target_xform = camera.get_global_transform()
	var local_transform_only_origin: = Transform3D(Basis(), get_global_transform().origin)
	var local_transform_only_basis: = Transform3D(get_global_transform().basis, Vector3())

	local_transform_only_origin = local_transform_only_origin.interpolate_with(target_xform, translate_factor)
	local_transform_only_basis = local_transform_only_basis.interpolate_with(target_xform, rotate_factor)
	set_global_transform(Transform3D(local_transform_only_basis.basis, local_transform_only_origin.origin))

func Follow():
	var local_transform_only_origin: = Transform3D(Basis(), get_global_transform().origin)
	var local_transform_only_basis: = Transform3D(get_global_transform().basis, Vector3())
	var target_xform: = camera.get_global_transform()

	local_transform_only_origin = target_xform
	local_transform_only_basis = target_xform
	set_global_transform(Transform3D(local_transform_only_basis.basis, local_transform_only_origin.origin))

func FOV(delta):

	if (gameData.isAiming
	&& !gameData.isRunning
	&& !gameData.isInspecting
	&& !gameData.isColliding
	&& !gameData.isReloading
	&&(gameData.weaponAction != "Manual")):
		camera.fov = lerp(camera.fov, gameData.aimFOV, delta * 50.0)


	elif (gameData.isAiming
	&& !gameData.isRunning
	&& !gameData.isInspecting
	&& !gameData.isColliding
	&&(gameData.weaponAction == "Manual")):
		camera.fov = lerp(camera.fov, gameData.aimFOV, delta * 50.0)


	else:
		camera.fov = lerp(camera.fov, gameData.baseFOV, delta * 25.0)

func DOF(delta):

	if (gameData.settings || gameData.interface):
		UIDOF(delta)

	elif gameData.aimFOV < 50 && gameData.isScoped && gameData.isAiming && !gameData.isColliding && !gameData.isInspecting && !gameData.isReloading:
		ScopeDOF(delta)

	elif gameData.isScoped && gameData.isAiming && !gameData.isColliding && !gameData.isInspecting:
		ScopeDOF(delta)

	else:
		ResetDOF(delta)

func UIDOF(delta):
	attribute.dof_blur_far_enabled = true
	attribute.dof_blur_near_enabled = true
	attribute.dof_blur_far_distance = 0.01
	attribute.dof_blur_far_transition = 5.0
	attribute.dof_blur_near_distance = 400
	attribute.dof_blur_near_transition = 1.0
	attribute.dof_blur_amount = move_toward(attribute.dof_blur_amount, 0.1, delta)

func ScopeDOF(delta):
	attribute.dof_blur_far_enabled = true
	attribute.dof_blur_near_enabled = false
	attribute.dof_blur_far_distance = 0.01
	attribute.dof_blur_far_transition = 5.0
	attribute.dof_blur_amount = move_toward(attribute.dof_blur_amount, 0.1, delta)

func ResetDOF(delta):
	attribute.dof_blur_amount = move_toward(attribute.dof_blur_amount, 0.0, delta)

	if attribute.dof_blur_amount == 0:
		attribute.dof_blur_far_enabled = false
		attribute.dof_blur_near_enabled = false
