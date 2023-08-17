Create embeddings for two chunk streams:
 - the actual file chunked
 - a forward interpolated version of the file chunked (processing each chunk by an LLM asking it to replace all the ambiguities of the current chunk with specifics from the previous context which are the last n chunks preceeding the current one)

Additionally, we can combine this with more traditional keyword based search.

Finally, we search each stream and combine the weights of each matched chunk. If one matches in the actual file chunk, the interpolated one, and the keyword search, it will have a higher weight than if it only matches in one of the streams.