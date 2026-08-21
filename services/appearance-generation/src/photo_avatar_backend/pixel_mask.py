from PIL import Image


def remove_edge_checkerboard(image: Image.Image) -> None:
    width, height = image.size
    pixels = image.load()
    candidates: set[tuple[int, int]] = set()
    for y in range(height):
        for x in range(width):
            red, green, blue, _ = pixels[x, y]
            if min(red, green, blue) >= 220 and max(red, green, blue) - min(red, green, blue) <= 24:
                candidates.add((x, y))

    pending = [
        point
        for point in candidates
        if point[0] in {0, width - 1} or point[1] in {0, height - 1}
    ]
    visited: set[tuple[int, int]] = set(pending)
    while pending:
        x, y = pending.pop()
        pixels[x, y] = (0, 0, 0, 0)
        for neighbor in ((x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)):
            if neighbor in candidates and neighbor not in visited:
                visited.add(neighbor)
                pending.append(neighbor)


def largest_component(alpha: bytes, width: int, height: int) -> int:
    parents: list[int] = []
    sizes: list[int] = []
    previous: list[tuple[int, int, int]] = []

    def find(index: int) -> int:
        root = index
        while parents[root] != root:
            root = parents[root]
        while parents[index] != index:
            parent = parents[index]
            parents[index] = root
            index = parent
        return root

    def union(left_index: int, right_index: int) -> None:
        left_root = find(left_index)
        right_root = find(right_index)
        if left_root == right_root:
            return
        if sizes[left_root] < sizes[right_root]:
            left_root, right_root = right_root, left_root
        parents[right_root] = left_root
        sizes[left_root] += sizes[right_root]

    for row in range(height):
        current: list[tuple[int, int, int]] = []
        offset = row * width
        column = 0
        while column < width:
            while column < width and alpha[offset + column] == 0:
                column += 1
            if column == width:
                break
            start = column
            while column < width and alpha[offset + column] > 0:
                column += 1
            end = column - 1
            run_index = len(parents)
            parents.append(run_index)
            sizes.append(end - start + 1)
            current.append((start, end, run_index))

        previous_index = 0
        for start, end, run_index in current:
            while previous_index < len(previous) and previous[previous_index][1] < start - 1:
                previous_index += 1
            overlap_index = previous_index
            while overlap_index < len(previous) and previous[overlap_index][0] <= end + 1:
                union(run_index, previous[overlap_index][2])
                overlap_index += 1
        previous = current

    if not parents:
        return 0
    return max(sizes[find(index)] for index in range(len(parents)))
