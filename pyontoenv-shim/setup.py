from __future__ import annotations

import os
from pathlib import Path

from setuptools import find_packages, setup

ROOT = Path(__file__).parent.resolve()

version = os.environ.get("ONTOENV_VERSION")
if not version:
    raise RuntimeError("ONTOENV_VERSION must be set when building pyontoenv shim")

readme = (
    (ROOT / "README.md").read_text(encoding="utf-8")
    if (ROOT / "README.md").exists()
    else "Compatibility wrapper that installs the ontoenv package."
)

setup(
    name="pyontoenv",
    version=version,
    description="Compatibility wrapper that depends on the ontoenv package.",
    long_description=readme,
    long_description_content_type="text/markdown",
    author="Gabe Fierro",
    author_email="gtfierro@mines.edu",
    url="https://github.com/gtfierro/ontoenv-rs",
    license="BSD-3-Clause",
    python_requires=">=3.9",
    install_requires=[f"ontoenv=={version}"],
    packages=find_packages(where="src"),
    package_dir={"": "src"},
    include_package_data=True,
    classifiers=[
        "License :: OSI Approved :: BSD License",
        "Programming Language :: Python",
        "Programming Language :: Python :: 3",
        "Programming Language :: Python :: 3 :: Only",
        "Programming Language :: Python :: 3.9",
        "Programming Language :: Python :: 3.10",
        "Programming Language :: Python :: 3.11",
        "Programming Language :: Python :: 3.12",
        "Programming Language :: Python :: 3.13",
    ],
)
