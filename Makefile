VERSION := 1.0.0

tag:
	git tag v${VERSION}

tag.push:
	git push origin v${VERSION}