#!/usr/bin/env python3
"""Train a GUI element classifier and export to ONNX.

Usage:
    # Train with RepVGG-Nano (default, recommended)
    python scripts/train_gui_classifier.py --data training-data/ --output models/gui-classifier.onnx

    # Train with MobileNet-v3-Small (fallback)
    python scripts/train_gui_classifier.py --data training-data/ --model mobilenet --output models/gui-classifier.onnx

    # Resume from checkpoint
    python scripts/train_gui_classifier.py --data training-data/ --resume checkpoints/best.pth

Prerequisites:
    pip install -r scripts/requirements-training.txt

The training-data/ directory should follow ImageNet-style layout:
    training-data/
    ├── Button/
    │   ├── img001.png
    │   └── img002.png
    ├── TextInput/
    │   └── ...
    └── Unknown/
        └── ...
"""

import argparse
import json
import sys
from pathlib import Path

import onnx
import onnxruntime as ort
import torch
import torch.nn as nn
import torchvision.transforms as T
from torch.utils.data import DataLoader, random_split
from torchvision.datasets import ImageFolder

# Label ordering must match OnnxGuiClassifier::LABELS in Rust
LABELS = [
    "Button", "TextInput", "Link", "MenuItem", "TabLabel",
    "StatusBar", "TitleBar", "ToolbarIcon", "TreeItem",
    "ScrollBar", "TextRegion", "Unknown",
]
NUM_CLASSES = len(LABELS)
INPUT_SIZE = 64


def build_repvgg_nano():
    """Custom RepVGG-Nano: 4 stages, channels [48, 96, 192, 384], ~0.8M params.

    Uses simple 3x3 conv blocks (no multi-branch during inference).
    For training, multi-branch (identity + 1x1 + 3x3) improves gradient flow,
    but for simplicity we use the deploy-mode architecture directly.
    """

    def conv_bn_relu(in_ch, out_ch, stride=1):
        return nn.Sequential(
            nn.Conv2d(in_ch, out_ch, 3, stride=stride, padding=1, bias=False),
            nn.BatchNorm2d(out_ch),
            nn.ReLU(inplace=True),
        )

    layers = []
    channels = [3, 48, 96, 192, 384]

    for i in range(4):
        # First block in each stage has stride=2 (downsampling)
        layers.append(conv_bn_relu(channels[i], channels[i + 1], stride=2))
        # Additional block (no downsampling)
        if i < 3:  # stages 0-2 get an extra block
            layers.append(conv_bn_relu(channels[i + 1], channels[i + 1]))

    model = nn.Sequential(
        *layers,
        nn.AdaptiveAvgPool2d(1),
        nn.Flatten(),
        nn.Linear(384, NUM_CLASSES),
    )
    return model


def build_mobilenet_v3():
    """MobileNet-v3-Small from timm, fine-tuned for 12 classes."""
    import timm

    model = timm.create_model(
        "mobilenetv3_small_100",
        pretrained=True,
        num_classes=NUM_CLASSES,
    )
    return model


def get_transforms(train=True):
    """Data augmentation and preprocessing transforms."""
    if train:
        return T.Compose([
            T.Resize((INPUT_SIZE, INPUT_SIZE)),
            T.RandomHorizontalFlip(p=0.3),
            T.ColorJitter(brightness=0.2, contrast=0.2, saturation=0.1),
            T.RandomRotation(5),
            T.ToTensor(),  # [0, 255] -> [0.0, 1.0], HWC -> CHW
        ])
    return T.Compose([
        T.Resize((INPUT_SIZE, INPUT_SIZE)),
        T.ToTensor(),
    ])


def train_one_epoch(model, loader, criterion, optimizer, device):
    model.train()
    total_loss = 0.0
    correct = 0
    total = 0

    for images, targets in loader:
        images, targets = images.to(device), targets.to(device)
        outputs = model(images)
        loss = criterion(outputs, targets)

        optimizer.zero_grad()
        loss.backward()
        optimizer.step()

        total_loss += loss.item() * images.size(0)
        _, predicted = outputs.max(1)
        correct += predicted.eq(targets).sum().item()
        total += targets.size(0)

    return total_loss / total, correct / total


@torch.no_grad()
def validate(model, loader, criterion, device):
    model.eval_mode = True
    model.eval()
    total_loss = 0.0
    correct = 0
    total = 0
    per_class_correct = [0] * NUM_CLASSES
    per_class_total = [0] * NUM_CLASSES

    for images, targets in loader:
        images, targets = images.to(device), targets.to(device)
        outputs = model(images)
        loss = criterion(outputs, targets)

        total_loss += loss.item() * images.size(0)
        _, predicted = outputs.max(1)
        correct += predicted.eq(targets).sum().item()
        total += targets.size(0)

        for t, p in zip(targets, predicted):
            per_class_total[t.item()] += 1
            if t.item() == p.item():
                per_class_correct[t.item()] += 1

    per_class_acc = {}
    for i, label in enumerate(LABELS):
        if per_class_total[i] > 0:
            per_class_acc[label] = per_class_correct[i] / per_class_total[i]

    return total_loss / total, correct / total, per_class_acc


def export_onnx(model, output_path, device):
    """Export model to ONNX format with verification."""
    model.eval()
    dummy = torch.randn(1, 3, INPUT_SIZE, INPUT_SIZE, device=device)

    torch.onnx.export(
        model,
        dummy,
        output_path,
        input_names=["input"],
        output_names=["output"],
        dynamic_axes=None,  # fixed batch size = 1
        opset_version=17,
    )

    # Verify ONNX model
    onnx_model = onnx.load(str(output_path))
    onnx.checker.check_model(onnx_model)

    # Sanity check with ONNX Runtime
    session = ort.InferenceSession(str(output_path))
    dummy_np = dummy.cpu().numpy()
    outputs = session.run(None, {"input": dummy_np})
    assert outputs[0].shape == (1, NUM_CLASSES), (
        f"Expected output shape (1, {NUM_CLASSES}), got {outputs[0].shape}"
    )

    print(f"ONNX model exported and verified: {output_path}")
    print(f"  File size: {output_path.stat().st_size / 1024:.1f} KB")


def remap_class_indices(dataset, label_order):
    """Remap ImageFolder class indices to match LABELS ordering.

    ImageFolder sorts classes alphabetically, but our ONNX model expects
    the specific ordering defined in LABELS.
    """
    folder_classes = dataset.classes  # alphabetically sorted by ImageFolder
    remap = {}
    for folder_idx, folder_name in enumerate(folder_classes):
        if folder_name in label_order:
            remap[folder_idx] = label_order.index(folder_name)
        else:
            print(f"  Warning: unknown class '{folder_name}' mapped to Unknown")
            remap[folder_idx] = label_order.index("Unknown")

    # Replace targets
    dataset.targets = [remap[t] for t in dataset.targets]
    for i in range(len(dataset.samples)):
        path, _ = dataset.samples[i]
        dataset.samples[i] = (path, remap[dataset.imgs[i][1]])

    dataset.classes = label_order
    dataset.class_to_idx = {name: idx for idx, name in enumerate(label_order)}
    return dataset


def main():
    parser = argparse.ArgumentParser(description="Train GUI element classifier")
    parser.add_argument("--data", required=True, help="Training data directory (ImageNet-style)")
    parser.add_argument("--output", default="models/gui-classifier.onnx", help="ONNX output path")
    parser.add_argument("--model", choices=["repvgg", "mobilenet"], default="repvgg")
    parser.add_argument("--epochs", type=int, default=30)
    parser.add_argument("--batch-size", type=int, default=32)
    parser.add_argument("--lr", type=float, default=1e-3)
    parser.add_argument("--resume", help="Resume from checkpoint")
    parser.add_argument("--device", default="auto", help="cpu, cuda, mps, or auto")
    args = parser.parse_args()

    # Device selection
    if args.device == "auto":
        if torch.cuda.is_available():
            device = torch.device("cuda")
        elif hasattr(torch.backends, "mps") and torch.backends.mps.is_available():
            device = torch.device("mps")
        else:
            device = torch.device("cpu")
    else:
        device = torch.device(args.device)
    print(f"Device: {device}")

    # Load dataset
    data_dir = Path(args.data)
    if not data_dir.exists():
        print(f"Error: data directory not found: {data_dir}", file=sys.stderr)
        sys.exit(1)

    dataset = ImageFolder(str(data_dir), transform=get_transforms(train=True))
    dataset = remap_class_indices(dataset, LABELS)

    n_total = len(dataset)
    n_val = max(1, int(n_total * 0.2))
    n_train = n_total - n_val
    train_set, val_set = random_split(dataset, [n_train, n_val])

    # Override val transforms (no augmentation)
    val_set.dataset = ImageFolder(str(data_dir), transform=get_transforms(train=False))
    val_set.dataset = remap_class_indices(val_set.dataset, LABELS)

    train_loader = DataLoader(train_set, batch_size=args.batch_size, shuffle=True, num_workers=2)
    val_loader = DataLoader(val_set, batch_size=args.batch_size, shuffle=False, num_workers=2)

    print(f"Dataset: {n_total} images ({n_train} train, {n_val} val)")
    print(f"Classes: {len(dataset.classes)}")

    # Build model
    if args.model == "repvgg":
        model = build_repvgg_nano()
        print("Model: RepVGG-Nano (~0.8M params)")
    else:
        model = build_mobilenet_v3()
        print("Model: MobileNet-v3-Small (~2.5M params)")

    if args.resume:
        model.load_state_dict(torch.load(args.resume, map_location=device, weights_only=True))
        print(f"Resumed from: {args.resume}")

    model = model.to(device)
    params = sum(p.numel() for p in model.parameters())
    print(f"Parameters: {params:,}")

    # Training setup
    criterion = nn.CrossEntropyLoss()
    optimizer = torch.optim.AdamW(model.parameters(), lr=args.lr, weight_decay=1e-4)
    scheduler = torch.optim.lr_scheduler.CosineAnnealingLR(optimizer, T_max=args.epochs)

    # Training loop
    best_val_acc = 0.0
    output_path = Path(args.output)
    output_path.parent.mkdir(parents=True, exist_ok=True)
    checkpoint_dir = Path("checkpoints")
    checkpoint_dir.mkdir(exist_ok=True)

    for epoch in range(args.epochs):
        train_loss, train_acc = train_one_epoch(model, train_loader, criterion, optimizer, device)
        val_loss, val_acc, per_class = validate(model, val_loader, criterion, device)
        scheduler.step()

        lr = scheduler.get_last_lr()[0]
        print(
            f"Epoch {epoch + 1:3d}/{args.epochs} | "
            f"Train: loss={train_loss:.4f} acc={train_acc:.3f} | "
            f"Val: loss={val_loss:.4f} acc={val_acc:.3f} | "
            f"LR: {lr:.6f}"
        )

        if val_acc > best_val_acc:
            best_val_acc = val_acc
            torch.save(model.state_dict(), checkpoint_dir / "best.pth")
            print(f"  New best val accuracy: {val_acc:.3f}")

    # Load best checkpoint and export
    model.load_state_dict(
        torch.load(checkpoint_dir / "best.pth", map_location=device, weights_only=True)
    )
    print(f"\nBest validation accuracy: {best_val_acc:.3f}")

    # Final per-class report
    _, _, per_class = validate(model, val_loader, criterion, device)
    print("\nPer-class accuracy:")
    for label, acc in sorted(per_class.items()):
        print(f"  {label:15s}: {acc:.3f}")

    # Export to ONNX
    export_onnx(model, output_path, device)

    # Save training metadata
    meta = {
        "model": args.model,
        "epochs": args.epochs,
        "best_val_accuracy": best_val_acc,
        "per_class_accuracy": per_class,
        "num_classes": NUM_CLASSES,
        "input_size": INPUT_SIZE,
        "labels": LABELS,
        "parameters": params,
    }
    meta_path = output_path.with_suffix(".json")
    meta_path.write_text(json.dumps(meta, indent=2))
    print(f"Training metadata saved: {meta_path}")


if __name__ == "__main__":
    main()
