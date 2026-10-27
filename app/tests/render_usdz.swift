// Renders a USD/USDZ file with SceneKit (Apple's USD import) to a PNG.
// Usage: swift render_usdz.swift <in.usdz> <out.png>
import AppKit
import SceneKit

let args = CommandLine.arguments
let scene = try SCNScene(url: URL(fileURLWithPath: args[1]), options: nil)
let (minB, maxB) = scene.rootNode.boundingBox
let center = SCNVector3((minB.x + maxB.x) / 2, (minB.y + maxB.y) / 2, (minB.z + maxB.z) / 2)
let size = max(maxB.x - minB.x, maxB.y - minB.y)
let camera = SCNNode()
camera.camera = SCNCamera()
camera.camera!.usesOrthographicProjection = true
camera.camera!.orthographicScale = Double(size) * 0.6
camera.position = SCNVector3(center.x, center.y, center.z + size * 2)
scene.rootNode.addChildNode(camera)
let light = SCNNode()
light.light = SCNLight()
light.light!.type = .directional
light.eulerAngles = SCNVector3(-0.6, 0.4, 0)
scene.rootNode.addChildNode(light)
scene.background.contents = NSColor(white: 0.95, alpha: 1)
let renderer = SCNRenderer(device: MTLCreateSystemDefaultDevice(), options: nil)
renderer.scene = scene
renderer.pointOfView = camera
renderer.autoenablesDefaultLighting = true
let image = renderer.snapshot(atTime: 0, with: CGSize(width: 900, height: 700), antialiasingMode: .multisampling4X)
let png = NSBitmapImageRep(data: image.tiffRepresentation!)!.representation(using: .png, properties: [:])!
try png.write(to: URL(fileURLWithPath: args[2]))
print("RENDERED \(args[2]) bounds=\(minB)...\(maxB)")
