// Reference kernels from UWASL/dedup-bench, copyright WASL 2023, Apache-2.0.
// Commit 8e2697cbf6332ac5da6dc615bfab82a720e820e4.
// License: ../../src/artifact/ram/LICENSE. Standalone timing wrapper added for Gutch.
#include <immintrin.h>
#include <cstdint>
#include <vector>
#include <string>
#include <fstream>
#include <iostream>
#include <algorithm>
#include <chrono>
#define AVX256_REGISTER_SIZE_BYTES 32
inline __m256i GreaterOrEqual8uAVX256(__m256i a, __m256i b) {
 return _mm256_cmpeq_epi8(_mm256_max_epu8(a,b),a);
}
uint8_t find_maximum_avx256(char *buff, uint64_t start_pos, uint64_t end_pos, __m256i *xmm_array){

    // Assume window_size is a multiple of AVX256_REGISTER_SIZE_BYTES for now
    // Assume num_vectors is even for now - True for most common window sizes. Can fix later via specific check

    uint64_t num_vectors = (end_pos - start_pos) / AVX256_REGISTER_SIZE_BYTES;

    uint64_t step = 2;
    uint64_t half_step = 1;

    // Load contents into __m256i structures
    // Could be optimized later as only 16 xmm registers are avaiable per CPU in 64-bit
    for(uint64_t i = 0; i < num_vectors; i++)
        xmm_array[i] = _mm256_loadu_si256((__m256i const *)(buff + start_pos + (AVX256_REGISTER_SIZE_BYTES * i)));

    // Repeat vmaxu until a single register is remaining with maximum values
    // Each iteration calculates maximums between a pair of registers and moves it into the first register in the pair
    // Finally, only one will be left with the maximum values from all pairs
    while(step <= num_vectors){

        for(uint64_t i = 0; i < num_vectors; i+=step)
            xmm_array[i] = _mm256_max_epu8(xmm_array[i], xmm_array[i+half_step]);

        // Multiply step by 2
        half_step = step;
        step = step << 1;

    }

    // Move the final set of values from the xmm into local memory
    uint8_t result_store[AVX256_REGISTER_SIZE_BYTES] = {0};

    _mm256_storeu_si256((__m256i *)&result_store, xmm_array[0]);

    // Sequentially scan the last remaining bytes (256 in this case) to find the max value
    uint8_t max_val = 0;
    for(uint64_t i = 0; i < AVX256_REGISTER_SIZE_BYTES; i++){
        if(result_store[i] > max_val)
            max_val = result_store[i];
    }

    // Return maximum value
    return max_val;
}
uint64_t range_scan_geq_avx256(char *buff, uint64_t start_position, uint64_t end_position, uint8_t target_value){

    uint64_t num_vectors = (end_position - start_position) / AVX256_REGISTER_SIZE_BYTES;
    uint64_t curr_scan_start;

    // Structures to store bytes from data stream and comparison results in 256-bit AVX format
    __m256i xmm_array, cmp_array;
    uint32_t cmp_mask;

    // Load max_value into xmm-format
    __m256i max_val_xmm = _mm256_set1_epi8((char)target_value);

    for(uint64_t i = 0; i < num_vectors; i++){
        curr_scan_start = start_position + (i * AVX256_REGISTER_SIZE_BYTES);
        // Load data into xmm register
        xmm_array = _mm256_loadu_si256((__m256i const *)(buff + curr_scan_start));

        /*
         Compare values with max_value. If a byte in xmm_array is geq max_val_xmm,
         ALL the corresponding bits of the corresponding byte in cmp_array are set to 1.
        */

        #if defined(__AVX512F__)
            cmp_mask = _mm256_cmpge_epu8_mask(xmm_array, max_val_xmm);
        #else
            cmp_array = GreaterOrEqual8uAVX256(xmm_array, max_val_xmm);

            // Create a mask using the most-significant bit of each byte value in cmp_array
            cmp_mask = _mm256_movemask_epi8(cmp_array);
        #endif

        // Return index of first non-zero bit in mask
        // This corresponds to the first non-zero byte in cmp_array
        if(cmp_mask){
            return curr_scan_start + (__builtin_ffs(cmp_mask) - 1);
        }
    }

    return end_position;
}
// Explicit scalar control: -O3 may auto-vectorize the scalar reference source.
__attribute__((noinline,optimize("no-tree-vectorize")))
uint64_t scalar_no_autovec(char* bytes, uint64_t size, uint64_t window) {
 uint8_t maximum=0;
 for(uint64_t i=0;i<window;i++) maximum=std::max(maximum,(uint8_t)bytes[i]);
 for(uint64_t i=window;i<size;i++) if((uint8_t)bytes[i]>=maximum) return i;
 return size;
}
uint64_t cut(char* bytes, uint64_t size, uint64_t window, __m256i* scratch, int mode) {
 size=std::min(size,window*4);
 if(size<=window) return size;
 if(mode==3) return scalar_no_autovec(bytes,size,window);
 uint8_t maximum=0;
 if(mode==0) {
  for(uint64_t i=0;i<window;i++) maximum=std::max(maximum,(uint8_t)bytes[i]);
  for(uint64_t i=window;i<size;i++) if((uint8_t)bytes[i]>=maximum) return i;
  return size;
 }
 maximum=find_maximum_avx256(bytes,0,window,scratch);
 auto result=range_scan_geq_avx256(bytes,window,size,maximum);
 if(mode==2 && result==size) {
  auto tail=window+((size-window)/32)*32;
  for(auto i=tail;i<size;i++) if((uint8_t)bytes[i]>=maximum) return i;
 }
 return result;
}
int main(int argc,char**argv) {
 if(argc!=3) return 2;
 std::ifstream paths(argv[1]);std::string path;std::vector<std::vector<char>> files;uint64_t total=0;
 while(std::getline(paths,path)) {
  if(!path.empty() && path.back()=='\r') path.pop_back();
  std::ifstream f(path,std::ios::binary|std::ios::ate);if(!f) return 3;
  auto size=f.tellg();f.seekg(0);std::vector<char> bytes((size_t)size);f.read(bytes.data(),size);
  total+=bytes.size();files.push_back(std::move(bytes));
 }
 std::ofstream out(argv[2]);out<<"[";bool first=true;
 for(uint64_t window:{8192,131072}) {
  std::vector<__m256i> scratch(window/32);
  std::vector<double> samples[4];uint64_t counts[4]={},fingerprints[4]={};
  for(int rep=0;rep<8;rep++) for(int offset=0;offset<4;offset++) {
   int mode=(rep+offset)%4;uint64_t count=0,fingerprint=0;
   auto start=std::chrono::steady_clock::now();
   for(auto& bytes:files) {
    asm volatile("" : : "g"(bytes.data()) : "memory");
    uint64_t pos=0;
    while(pos<bytes.size()) {
     auto length=cut(bytes.data()+pos,bytes.size()-pos,window,scratch.data(),mode);
     pos+=length;count++;fingerprint=fingerprint*31+length;
    }
   }
   double seconds=std::chrono::duration<double>(std::chrono::steady_clock::now()-start).count();
   counts[mode]=count;fingerprints[mode]=fingerprint;if(rep) samples[mode].push_back(seconds);
  }
  for(int mode=0;mode<4;mode++) {
   std::sort(samples[mode].begin(),samples[mode].end());
   if(!first)out<<",";first=false;
   out<<"{\"window\":"<<window<<",\"variant\":\""<<std::vector<std::string>{"reference-scalar-source-O3","reference-avx2","reference-avx2-fixed-tail","reference-scalar-no-autovec"}[mode]
      <<"\",\"bytes\":"<<total<<",\"chunks\":"<<counts[mode]<<",\"boundary_fingerprint\":"<<fingerprints[mode]
      <<",\"seconds\":"<<samples[mode][3]<<",\"samples\":[";
   for(int i=0;i<7;i++){if(i)out<<",";out<<samples[mode][i];}out<<"]}";
  }
  std::cerr<<"Completed window "<<window<<"\n";
 }
 out<<"]";
}
