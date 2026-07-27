local function make_tree(depth)
    if depth == 0 then
        return nil
    end
    return { left = make_tree(depth - 1), right = make_tree(depth - 1) }
end

local function node_count(n)
    if n == nil then
        return 0
    end
    return 1 + node_count(n.left) + node_count(n.right)
end

local function main()
    local _t0 = os.clock()
    local min_depth = 4
    local max_depth = 14

    local stretch = make_tree(max_depth + 1)
    print(node_count(stretch))

    local long_lived = make_tree(max_depth)

    local total_check = 0
    local depth = min_depth
    while depth <= max_depth do
        local iterations = 1 << (max_depth - depth + min_depth)
        local check = 0
        for _ = 1, iterations do
            check = check + node_count(make_tree(depth))
        end
        total_check = total_check + check
        depth = depth + 2
    end

    print(node_count(long_lived))
    print(total_check)
    io.stderr:write(string.format("bench_ns=%.0f\n", (os.clock() - _t0) * 1e9))
end

main()
