max_size = 4 * 1024 * 1024 * 1024

num = 0
with open("nvme_byte_pattern", "wb") as file:
    for _ in range(0, max_size):
        file.write(num.to_bytes())
        num += 1
        num = num % 251 # mod prime to ensure that each logical block starts at a different value

