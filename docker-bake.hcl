// Docker Bake configuration for chasm-server multi-arch builds.
//
// Usage:
//   docker buildx bake               # builds both linux/amd64 + linux/arm64 to OCI tarball
//   docker buildx bake local         # builds native arch only, loads into local docker daemon
//   docker buildx bake amd64         # builds linux/amd64 only, loads
//   docker buildx bake arm64         # builds linux/arm64 only, loads (uses QEMU on non-arm64 hosts)
//   docker buildx bake push          # builds both arches, pushes manifest to registry (set REGISTRY)

variable "REGISTRY" {
  default = ""
}

variable "TAG" {
  default = "latest"
}

variable "IMAGE_NAME" {
  default = "chasm"
}

function "image" {
  params = [tag]
  result = REGISTRY == "" ? "${IMAGE_NAME}:${tag}" : "${REGISTRY}/${IMAGE_NAME}:${tag}"
}

group "default" {
  targets = ["multi"]
}

target "_common" {
  context    = "."
  dockerfile = "Dockerfile"
}

target "multi" {
  inherits  = ["_common"]
  platforms = ["linux/amd64", "linux/arm64"]
  tags      = [image(TAG)]
  output    = ["type=oci,dest=./dist/chasm-multi.tar"]
}

target "push" {
  inherits  = ["_common"]
  platforms = ["linux/amd64", "linux/arm64"]
  tags      = [image(TAG)]
  output    = ["type=registry"]
}

target "local" {
  inherits = ["_common"]
  tags     = [image("local")]
  output   = ["type=docker"]
}

target "amd64" {
  inherits  = ["_common"]
  platforms = ["linux/amd64"]
  tags      = [image("amd64")]
  output    = ["type=docker"]
}

target "arm64" {
  inherits  = ["_common"]
  platforms = ["linux/arm64"]
  tags      = [image("arm64")]
  output    = ["type=docker"]
}
