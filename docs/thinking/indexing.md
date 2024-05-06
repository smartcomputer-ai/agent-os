# Indexing 

There are two, somewhat related, albeit seprable, indexing problems in agent os.

The first prpblem is very grit specific: large trees will get slow to modify and load (once in memory the dictionary is relatively fast). Plus, we cannot save tree collections that are larger than memory, e.g., many millions of sub items (trees or blobs).

The second problem is indexing fields inside, say, blobs, and making those searchable, and filterable. Vector indexes are a subproblem of this, but not the only one. This problem becomes more atenuated once we have large collections inside grit, but indexes could also be required of just a few large blon items, with say, large jsons inside (or CSVs, or similar). So in many ways this is orthogonal to the first problem.

The first problem is more akin to a primary key index and the second problem is more akin to a secondary index.

My sense is that the solution is to solve these two problems separately. Once for large grit trees (in a grit native way) and once for various sub indexes (in a more general way).

## Grit Indexing

The most straight forward solution is to use some sort of tree sharding or partitioning. Define a partitioning function for the tree keys and then partition the data into multiple sub-trees that are managed by the paritioner module.

The partitioning can be recursive more like a trie, where a piece of the main key is taken and then the tree is split into multiple sub-trees. This is a bit like a radix tree, but with a more general partitioning function. The problem with this approach is the need for rebalancing.

The actual structure we want is closest to a HAMT (https://en.wikipedia.org/wiki/Hash_array_mapped_trie)